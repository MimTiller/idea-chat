#!/usr/bin/env node
// Idea Chat roles and permissions, over HTTP, against a real server.
//
// The unit tests cover the rules on in-memory spaces; this drives the server
// functions themselves (`POST /_srv/<fn>`, a JSON array of the arguments, the
// session as a bearer) so the handlers are checked to call the rules, return
// the right refusals, and act on the database.
//
//   CHAT_URL=http://127.0.0.1:8090 \
//   CHAT_DB_URL=postgres://postgres@127.0.0.1:5432/chat \
//     node tools/chat-roles-flows.js
//
// CHAT_DB_URL is the server's own database: accounts are made through the API
// and marked verified there with psql, so no mail has to be read.
const { execFileSync } = require("child_process");

const BASE = process.env.CHAT_URL || "http://127.0.0.1:8090";
const DB = process.env.CHAT_DB_URL;
if (!DB) {
  console.error("CHAT_DB_URL is required (the server's database, for psql)");
  process.exit(2);
}

// ---- permission bits: crates/chat-model/src/perms.rs ------------------------------
const NAMES = [
  "ViewChannel", "SendMessage", "EditOwnMessage", "DeleteOwnMessage", "DeleteOthersMessage", "ManagePins",
  "AddReaction", "MentionEveryone", "AttachFile", "EmbedLink", "VoiceConnect", "VoiceSpeak", "VoiceVideo",
  "VoiceShareScreen", "VoiceForceMute", "VoiceMoveMember", "ManageChannels", "ManageChannelRoles", "ManageInvites",
  "ManageRoles", "ManageMembers", "ManageSpaceSettings", "BanMembers", "TimeoutMembers",
];
const P = {};
NAMES.forEach((n, i) => (P[n] = 1n << BigInt(i)));
P.IsAdmin = 1n << 63n;
const or = (...xs) => xs.reduce((a, b) => a | b, 0n);
// What a new space's everyone baseline holds (perms::DEFAULT_EVERYONE).
const DEFAULT_EVERYONE = or(P.ViewChannel, P.SendMessage, P.EditOwnMessage, P.DeleteOwnMessage, P.AddReaction,
  P.AttachFile, P.EmbedLink, P.VoiceConnect, P.VoiceSpeak, P.VoiceVideo, P.VoiceShareScreen);

// ---- wire --------------------------------------------------------------------------
// JSON with exact u64s: permission sets reach bit 63, past what a JS number holds.
function enc(v) {
  if (typeof v === "bigint") return v.toString();
  if (Array.isArray(v)) return "[" + v.map(enc).join(",") + "]";
  if (v && typeof v === "object") return "{" + Object.entries(v).map(([k, x]) => JSON.stringify(k) + ":" + enc(x)).join(",") + "}";
  return JSON.stringify(v === undefined ? null : v);
}

async function post(name, args, cookie) {
  const res = await fetch(`${BASE}/_srv/${name}`, {
    method: "POST",
    headers: { "content-type": "application/json", ...(cookie ? { cookie } : {}) },
    body: enc(args),
  });
  return { res, text: await res.text() };
}

// The answer is {"Ok": value} or {"Err": {"Failed": {"why", "message"}}} (HTTP 200); anything
// else (a 4xx with a plain-text body, a malformed request) is a transport failure, never a
// permission refusal.
async function call(name, args, cookie, tries = 4) {
  for (let attempt = 0; ; attempt++) {
    const { res, text } = await post(name, args, cookie);
    // The server's own rate limit answers 429 in plain text: wait it out and ask again.
    if (res.status === 429 && attempt < tries + 4) {
      const wait = Number((/retry in (\d+)s/.exec(text) || [])[1] || 10);
      await new Promise((r) => setTimeout(r, (wait + 2) * 1000));
      continue;
    }
    let body;
    try { body = JSON.parse(text); } catch (e) { body = null; }
    if (body && body.Ok !== undefined) return { ok: true, value: body.Ok, res };
    if (body && body.Err) {
      const [kind, detail] = Object.entries(body.Err)[0] || ["error", {}];
      const why = (detail && detail.why) || kind;
      const message = (detail && detail.message) || "";
      if (attempt < tries && /too many|rate limit|slow down|try again in/i.test(why + " " + message)) {
        await new Promise((r) => setTimeout(r, 15000));
        continue;
      }
      return { ok: false, why, message, res };
    }
    return { ok: false, why: "transport", message: `${res.status} ${text.slice(0, 140)}`, res };
  }
}

// ---- assertions --------------------------------------------------------------------
let total = 0, failed = 0;
function report(pass, label, detail) {
  total++;
  if (!pass) failed++;
  console.log(`${pass ? "  PASS" : "  FAIL"}  ${label}${!pass && detail ? `\n          ${detail}` : ""}`);
}
const ok = (label, r) => report(r.ok, label, r.ok ? "" : `refused: ${r.why}: ${r.message}`);
const refused = (label, r) => {
  const real = !r.ok && r.why !== "transport";
  report(real, label + (real ? `   [${r.why}: ${r.message}]` : ""),
    r.ok ? `was allowed: ${JSON.stringify(r.value).slice(0, 100)}` : `not a refusal, a transport failure: ${r.message}`);
};
const section = (t) => console.log(`\n${t}`);

// ---- accounts ----------------------------------------------------------------------
const tag = Math.random().toString(36).slice(2, 8);
function psql(sql) {
  execFileSync("psql", [DB, "-q", "-c", sql], { stdio: ["ignore", "ignore", "inherit"] });
}
async function account(name) {
  const email = `${name}-${tag}@example.com`;
  const password = "correct horse battery staple";
  const handle = `${name}${tag}`;
  const up = await call("sign_up", [email, password, handle, name.toUpperCase()]);
  if (!up.ok) throw new Error(`sign_up ${name}: ${up.why}: ${up.message}`);
  psql(`UPDATE users SET email_verified_at = now() WHERE email = '${email}'`);
  const session = await call("sign_in", [email, password]);
  if (!session.ok) throw new Error(`sign_in ${name}: ${session.why}: ${session.message}`);
  const cookie = session.res.headers.getSetCookie().map((c) => c.split(";")[0]).join("; ");
  const id = session.value.me.user.id;
  if (!id || !cookie) throw new Error(`no id or session cookie for ${name}`);
  return { name, cookie, id };
}
function findKey(o, key) {
  if (!o || typeof o !== "object") return undefined;
  if (key in o && typeof o[key] === "string") return o[key];
  for (const v of Object.values(o)) { const r = findKey(v, key); if (r) return r; }
  return undefined;
}

// ---- helpers over the verbs --------------------------------------------------------
const as = (who) => (name, args) => call(name, args, who.cookie);
async function role(by, space, name, perms) {
  const r = await by("create_role", [space, name, perms]);
  return { r, id: r.ok ? r.value.id : null };
}
async function channel(by, space, name) {
  const r = await by("create_channel", [space, "text", name, null]);
  return { r, id: r.ok ? r.value.id : null };
}
const send = (by, ch, text = "hi") => by("send", [ch, text, null, []]);
const history = (by, ch) => by("history", [ch, { at: "latest" }]);

(async () => {
  section("setup: four people, a space, two channels, the seeded roles");
  const [owner, mgr, bob, cat] = [await account("owner"), await account("mgr"), await account("bob"), await account("cat")];
  const O = as(owner), M = as(mgr), B = as(bob), C = as(cat);
  const sp = await O("create_space", ["Roles e2e"]);
  ok("the owner makes a space", sp);
  const S = sp.value;
  const inv = await O("create_invite", [S, null, null]);
  ok("the owner makes an invite", inv);
  const code = inv.ok ? inv.value.code : null;
  for (const [who, fn] of [[mgr, M], [bob, B], [cat, C]]) ok(`${who.name} joins`, await fn("join_space", [code]));
  const seeded = await O("space_roles", [S]);
  const byName = Object.fromEntries((seeded.ok ? seeded.value : []).map((r) => [r.name, r.id]));
  report(seeded.ok && byName.Admin && byName.Moderator, "the space seeded Admin and Moderator");
  ok("the owner hands cat the seeded Admin", await O("set_roles", [S, cat.id, [byName.Admin]]));
  const room = (await channel(O, S, "room")).id;
  report(!!room, "the owner makes #room");

  section("1. what an ordinary member may and may not do (the default baseline)");
  const first = await send(B, room, "hello from bob");
  ok("a member posts in an open channel", first);
  const firstId = first.ok ? first.value.id : null;
  ok("a member reads it back", await history(B, room));
  refused("a member cannot create a role", await B("create_role", [S, "x", P.SendMessage]));
  refused("a member cannot set the everyone baseline", await B("set_everyone", [S, P.ViewChannel]));
  refused("a member cannot ban", await B("ban_member", [S, cat.id]));
  refused("a member cannot time anyone out", await B("timeout_member", [S, cat.id, 5]));
  refused("a member cannot remove anyone", await B("remove_member", [S, cat.id]));
  refused("a member cannot make a channel", await B("create_channel", [S, "text", "nope", null]));
  refused("a member cannot make an invite", await B("create_invite", [S, null, null]));
  refused("a member cannot rename the space", await B("rename_space", [S, "mine now"]));
  refused("a member cannot hand out the seeded Moderator", await B("set_roles", [S, cat.id, [byName.Moderator]]));
  ok("the Admin holds the space open: she makes a channel", (await channel(C, S, "staff")).r);

  section("2. the seeded Moderator: what its powers do and do not reach");
  ok("the owner hands the seeded Moderator to mgr", await O("set_roles", [S, mgr.id, [byName.Moderator]]));
  const helper = await role(O, S, "Helper", P.SendMessage);
  ok("the owner makes a Helper role within their powers", helper.r);
  ok("the Admin hands that junior role to a member", await C("set_roles", [S, bob.id, [helper.id]]));
  ok("the Admin can edit a junior role", await C("update_role", [S, helper.id, "Helper", or(P.SendMessage, P.AddReaction)]));
  refused("a Moderator creates no role (ManageRoles is not in the seeded set)", await M("create_role", [S, "quiet", P.AddReaction]));
  refused("a Moderator edits no role of their own rank", await M("update_role", [S, byName.Moderator, "Moderator", P.BanMembers]));
  refused("a Moderator cannot set the everyone baseline (no ManageRoles)", await M("set_everyone", [S, P.ViewChannel]));
  refused("a Moderator cannot make a channel (ManageChannels is not in the seeded set)", await M("create_channel", [S, "text", "quiet", null]));

  section("3. timeout and ban");
  ok("a Moderator times bob out for 5 minutes", await M("timeout_member", [S, bob.id, 5]));
  refused("a timed-out member cannot post", await send(B, room, "i am timed out"));
  ok("a Moderator lifts the timeout", await M("timeout_member", [S, bob.id, null]));
  ok("and the member posts again", await send(B, room, "back"));
  ok("a Moderator bans bob", await M("ban_member", [S, bob.id]));
  refused("a banned member cannot post", await send(B, room, "i am banned"));
  refused("a banned member cannot rejoin with the invite", await B("join_space", [code]));
  ok("a Moderator unbobs them", await M("unban_member", [S, bob.id]));
  ok("and they rejoin", await B("join_space", [code]));

  section("4. the seeded Admin reaches down where the Moderator cannot");
  refused("a Moderator cannot time out the Admin", await M("timeout_member", [S, cat.id, 5]));
  refused("a Moderator cannot ban the Admin", await M("ban_member", [S, cat.id]));
  refused("a Moderator cannot remove the Admin", await M("remove_member", [S, cat.id]));
  refused("a Moderator cannot strip the Admin's roles", await M("set_roles", [S, cat.id, []]));
  refused("a Moderator cannot hand out the Admin role above them", await M("set_roles", [S, bob.id, [byName.Admin]]));
  ok("the Admin hands bob the seeded Moderator (a normal role, handed out)", await C("set_roles", [S, bob.id, [byName.Moderator]]));

  section("5. the everyone baseline");
  ok("the owner narrows the baseline to view only", await O("set_everyone", [S, P.ViewChannel]));
  refused("then a Moderator cannot post (Moderator has no SendMessage, a real gate)", await send(M, room, "read only now"));
  ok("but can still read", await history(M, room));
  if (firstId) refused("and cannot react (AddReaction left the baseline)", await M("react", [firstId, "\ud83d\udc45", true]));
  refused("a Moderator cannot widen the baseline beyond what they hold", await M("set_everyone", [S, or(P.ViewChannel, P.SendMessage)]));
  refused("nobody can put the admin tag in the baseline", await O("set_everyone", [S, P.IsAdmin]));
  ok("the owner takes the baseline to nothing", await O("set_everyone", [S, 0n]));
  refused("then a Moderator cannot even read (ViewChannel is a real gate)", await history(M, room));
  ok("the owner restores the default baseline", await O("set_everyone", [S, DEFAULT_EVERYONE]));
  ok("and the Moderator posts again", await send(M, room, "restored"));

  section("6. per-channel overwrites");
  const muted = await role(O, S, "Muted", 0n);
  ok("the owner makes an empty role", muted.r);
  ok("bob holds it beside Moderator", await O("set_roles", [S, bob.id, [byName.Moderator, muted.id]]));
  ok("deny SendMessage for that role in #room", await O("set_overwrite", [room, muted.id, 0n, P.SendMessage]));
  refused("the role cannot post there", await send(B, room, "muted"));
  ok("the Moderator alone still can", await send(M, room, "not muted"));
  ok("the overwrite is cleared", await O("clear_overwrite", [room, muted.id]));
  ok("and the role posts again", await send(B, room, "unmuted"));
  ok("deny ViewChannel for that role in #room", await O("set_overwrite", [room, muted.id, 0n, P.ViewChannel]));
  refused("the role cannot read #room", await history(B, room));
  ok("the Moderator alone can read it", await history(M, room));
  ok("the overwrite is cleared again", await O("clear_overwrite", [room, muted.id]));

  section("7. an announcements channel only one role can post in");
  const ann = (await channel(O, S, "announce")).id;
  const poster = await role(O, S, "Poster", 0n);
  ok("a Poster role, given to bob", await O("set_roles", [S, bob.id, [poster.id]]));
  ok("the baseline loses SendMessage", await O("set_everyone", [S, or(P.ViewChannel, P.AddReaction)]));
  ok("the Poster role may post in #announce", await O("set_overwrite", [ann, poster.id, P.SendMessage, 0n]));
  ok("a Poster posts in #announce", await send(B, ann, "news"));
  refused("a Moderator cannot post in #announce (no SendMessage in their set)", await send(M, ann, "no"));
  refused("a Poster cannot post in #room (the overwrite is only on #announce)", await send(B, room, "no"));
  ok("the owner restores the baseline", await O("set_everyone", [S, DEFAULT_EVERYONE]));
  ok("the owner clears bob's role", await O("set_roles", [S, bob.id, []]));

  section("8. who may set overwrites");
  const chanRole = await role(O, S, "ChannelWarden", or(P.ManageChannelRoles));
  const later = await role(O, S, "Later", 0n);
  ok("the owner makes a warden role, then a role below it", chanRole.r && later.r ? { ok: true } : { ok: false, why: "setup", message: "roles" });
  ok("the owner gives bob the warden role", await O("set_roles", [S, bob.id, [chanRole.id]]));
  ok("a warden mutes a role below their rank", await B("set_overwrite", [room, later.id, 0n, P.SendMessage]));
  refused("a warden cannot touch a role above their rank", await B("set_overwrite", [room, muted.id, 0n, P.SendMessage]));
  refused("a warden cannot allow a power they lack", await B("set_overwrite", [room, later.id, P.ManageMembers, 0n]));
  refused("a Moderator cannot set one (ManageChannelRoles is not in the seeded set)", await M("set_overwrite", [room, later.id, 0n, P.SendMessage]));
  ok("the warden clears theirs", await B("clear_overwrite", [room, later.id]));

  console.log(`\n${total} assertions, ${failed} failed`);
  const roles = await O("space_roles", [S]);
  if (roles.ok) console.log("roles at the end:", JSON.stringify(roles.value).slice(0, 400));
  process.exit(failed ? 1 : 0);
})().catch((e) => {
  console.error("the script itself failed:", e.message);
  process.exit(2);
});
