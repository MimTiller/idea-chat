#!/usr/bin/env node
// Per-space custom emoji and stickers, end to end against a fresh server.
//
// The claims worth checking outside the process: that a member's live socket
// restates the emoji and sticker sets when the admin renames or adds one
// (metadata in the clear, so the member must see it), that a plain member is
// refused the manage verbs with a readable sentence, that an oversize image
// is refused at begin_upload, that a sticker-only message sends with an empty
// body, and that a foreign space's sticker is refused at send.
//
//   cargo build -p chat-server
//   node tools/chat-expressions-flows.js
//
// Expects a FRESH database: it creates one, drops it at the end. S3 (Garage
// on this host at :59000) must be live for the uploads.
const { execFileSync, spawn } = require('child_process');
const fs = require('fs');

const HOME = process.env.HOME;
const REPO = process.env.CHECKOUT || '/home/ollama2/idea-chat-roles-docs';
const CHATBIN = process.env.CHATBIN || '/home/ollama2/idea-chat-roles-docs/target/debug/chat-server';

const PORT = 19000 + (Math.floor(Math.random() * 900));
const API = `http://localhost:${PORT}/_srv`;
const WS = `ws://localhost:${PORT}/_srv/_ws/live`;
const LOG = `/tmp/chat-expressions-${PORT}.log`;

let pass = 0, fail = 0;
const check = (name, ok, detail) => {
  if (ok) { console.log(`  ok   ${name}`); pass++; }
  else { console.log(` FAIL  ${name}`); if (detail) console.log(`        ${detail}`); fail++; }
};
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

const psql = (q) => execFileSync('psql', ['postgres://postgres@127.0.0.1:5432/', '-c', q, '-q']).toString();

const call = async (fn, args, jar) => {
  const headers = { 'Content-Type': 'application/json' };
  if (jar) headers.Cookie = jar;
  const r = await fetch(`${API}/${fn}`, { method: 'POST', headers, body: JSON.stringify(args) });
  const setCookie = r.headers.get('set-cookie');
  const raw = await r.text();
  let body;
  try { body = JSON.parse(raw); } catch { body = { Err: raw.trim() }; }
  return { body, cookie: setCookie ? setCookie.split(',')[0].split(';')[0] : null, raw };
};

// Sign up, read the mailer's printed code from the server log, verify.
const account = async (name) => {
  const tag = Date.now().toString(36) + Math.random().toString(36).slice(2, 6);
  const email = `${name}-${tag}@example.com`.toLowerCase();
  const up = await call('sign_up', [email, 'password12345', `${name}${tag}`.slice(0, 20), name]);
  if (!up.body.Ok) throw new Error(`sign_up ${name}: ${JSON.stringify(up.body)}`);
  let code;
  for (let i = 0; i < 40 && !code; i++) {
    await sleep(250);
    const line = fs.readFileSync(LOG, 'utf8').split('\n').reverse().find((l) => l.includes(`to ${email}`));
    code = line && (line.match(/\b(\d{6})\b/) || [])[1];
  }
  if (!code) throw new Error(`no code for ${email} in ${LOG}`);
  const v = await call('verify_email', [email, code]);
  if (!v.body.Ok) throw new Error(`verify ${name}: ${JSON.stringify(v.body)}`);
  return { jar: v.cookie, id: v.body.Ok.me.user.id };
};

async function main() {
  // A database no other run shares, and a freshly built server.
  const db = `chat_expr_${Date.now()}_${process.pid}`;
  try { psql(`create database ${db}`); } catch (e) { console.log(`FLOW RESULT: 0 PASS, 1 FAIL (could not create db: ${e})`); return; }
  try {
    execFileSync('sh', ['-c', `. \$HOME/.cargo/env && cd ${REPO} && cargo build -p chat-server`], { stdio: 'inherit' });
  } catch { console.log(`FLOW RESULT: 0 PASS, 1 FAIL (chat-server build failed)`); try { psql(`drop database ${db}`); } catch {} return; }

  const s3 = fs.readFileSync(`${HOME}/s3.env`, 'utf8').split('\n')
    .filter((l) => /^[A-Z0-9_]+=/.test(l))
    .reduce((o, l) => { const i = l.indexOf('='); if (l[i - 1] !== '"' && l.endsWith('"')) { const k = l.slice(0, i); o[k] = l.slice(i + 2, -1); } else { o[l.slice(0, i)] = l.slice(i + 1); } return o; }, {});
  const env = { ...process.env, ...s3,
    CHAT_BIND: `127.0.0.1:${PORT}`,
    CHAT_PUBLIC_URL: `http://localhost:${PORT}`,
    DATABASE_URL: `postgres://postgres@127.0.0.1:5432/${db}`,
    CHAT_INSTANCE: `expressions-flow-${db}`,
    CHAT_MAILER: 'log',
    CALL_SECRET: 'flow-call-secret-change-me-0123456789',
    LIVE_SECRET: 'flow-live-secret-change-me-0123456789',
  };
  const log = fs.openSync(LOG, 'w');
  const server = spawn(CHATBIN, { env, stdio: ['ignore', log, log] });
  let ready = false;
  for (let i = 0; i < 30 && !ready; i++) {
    await sleep(1000);
    try { const r = await fetch(`http://localhost:${PORT}/`, { maxRedirects: 0 }); ready = true; } catch {}
    if (!ready) {
      try { process.kill(server.pid, 0); } catch { break; }
    }
  }
  check('the server boots and answers', ready, ready ? '' : `see ${LOG}`);
  if (!ready) { console.log(`FLOW RESULT: ${pass} PASS, ${fail} FAIL`); try { psql(`drop database ${db}`); } catch {}; return; }

  // First signer-up is the instance's admin; the second is a plain member.
  let admin, member;
  try { admin = await account('boss'); } catch (e) { console.log(`FLOW RESULT: ${pass} PASS, ${fail} FAIL (admin signup: ${e})`); return cleanup(server); }
  check('the admin signs up and verifies', !!admin.jar, '');

  const sp = await call('create_space', ['Expressions'], admin.jar);
  check('the admin creates a space', !!sp.body.Ok, JSON.stringify(sp.body.Err || {}));
  const spaceId = sp.body.Ok;
  const inv = await call('create_invite', [spaceId, null, null], admin.jar);
  check('the admin mints an invite', !!inv.body.Ok, JSON.stringify(inv.body.Err || {}));
  const code = inv.body.Ok && inv.body.Ok.code;

  try { member = await account('peon'); } catch (e) { console.log(`FLOW RESULT: ${pass} PASS, ${fail} FAIL (member signup: ${e})`); return cleanup(server); }
  const joined = await call('join_space', [code], member.jar);
  check('a plain member joins through the invite', !!joined.body.Ok, JSON.stringify(joined.body.Err || {}));

  // Uploads: an emoji image and a sticker image through begin/PUT/finish.
  const png = Buffer.from('iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAYAAAAfFcSJAAAADUlEQVR42mNk+M9QDwADhgWAW9zvM1AAAAD5ZlEQAAAAEAAAAEAAAAESGV9sAAAABJRU5ErkJggg==', 'base64');
  const uploadImage = async (fn, purpose) => {
    const b = await call('begin_upload', [fn, png.length, 'image/png', purpose], admin.jar);
    if (!b.body.Ok) return { begin: b };
    const slot = b.body.Ok;
    let put = { status: 0 };
    try { put = await fetch(slot.put_url, { method: 'PUT', body: png, headers: { 'Content-Type': slot.content_type } }); } catch {}
    const f = await call('finish_upload', [slot.id, 1, 1], admin.jar);
    return { begin: b, put, finish: f, slot: slot.id };
  };
  const em = await uploadImage('grin.png', 'emoji');
  check('the emoji upload opens (purpose emoji)', !!em.begin.body.Ok, JSON.stringify(em.begin.body.Err || {}));
  check('the emoji bytes land in the slot', em.put && em.put.status === 200, '');
  check('the emoji upload finishes', !!em.finish.body.Ok, JSON.stringify((em.finish || {}).body.Err || {}));
  const st = await uploadImage('fire.png', 'sticker');
  check('the sticker upload opens (purpose sticker)', !!st.begin.body.Ok, JSON.stringify(st.begin.body.Err || {}));
  check('the sticker upload finishes', !!st.finish.body.Ok, JSON.stringify((st.finish || {}).body.Err || {}));

  const ce = await call('create_emoji', [spaceId, 'grin', em.slot], admin.jar);
  check('the admin adds the custom emoji', !!ce.body.Ok, JSON.stringify(ce.body.Err || {}));
  const cs = await call('create_sticker', [spaceId, 'fire', 'a flame', st.slot], admin.jar);
  check('the admin adds a sticker', !!cs.body.Ok, JSON.stringify(cs.body.Err || {}));
  const emojiId = ce.body.Ok && ce.body.Ok.id;
  const stickerId = cs.body.Ok && cs.body.Ok.id;

  const lm = await call('list_emoji', [spaceId], member.jar);
  check('the member lists the emoji (membership is enough)', !!lm.body.Ok && (lm.body.Ok || []).some((e) => e.name === 'grin'), JSON.stringify(lm.body.Err || {}));
  const ls = await call('list_stickers', [spaceId], member.jar);
  check('the member lists the sticker', !!ls.body.Ok && (ls.body.Ok || []).some((s) => s.name === 'fire'), JSON.stringify(ls.body.Err || {}));

  // A plain member is refused the manage verbs, with the readable sentence.
  const refused = await call('create_emoji', [spaceId, 'hacked', em.slot], member.jar);
  check('the member cannot add an emoji', !refused.body.Ok, JSON.stringify(refused.body.Ok || {}));
  check('with the manage-custom-emoji sentence', (refused.raw || '').includes('Only those who manage custom emoji can add one.'), JSON.stringify(refused.body.Err || {}));
  const refused2 = await call('create_sticker', [spaceId, 'hacked', 'x', st.slot], member.jar);
  check('the member cannot add a sticker', !refused2.body.Ok, JSON.stringify(refused2.body.Ok || {}));
  check('with the manage-stickers sentence', (refused2.raw || '').includes('Only those who manage stickers can add one.'), JSON.stringify(refused2.body.Err || {}));

  // begin_upload enforces the per-image byte limit.
  const big = await call('begin_upload', ['huge.png', 300 * 1024, 'image/png', 'emoji'], admin.jar);
  check('an over-emoji-limit upload is refused', !big.body.Ok, JSON.stringify(big.body.Ok || {}));
  check('naming the 256 KiB limit', (big.raw || '').includes('Files can be up to 256 KiB.'), JSON.stringify(big.body.Err || {}));

  // The member's live socket: snapshot carries the sets; restatements follow.
  const lt = await call('live_ticket', [null], member.jar);
  check('the member mints a live ticket', !!lt.body.Ok, JSON.stringify(lt.body.Err || {}));
  if (lt.body.Ok) {
    const argsHex = Buffer.from(JSON.stringify([lt.body.Ok]), 'utf8').toString('hex');
    const ws = new WebSocket(`${WS}?args=${argsHex}`);
    const frames = [];
    ws.addEventListener('message', (m) => { const d = m.data; if (typeof d === 'string') try { frames.push(JSON.parse(d)); } catch {} });
    const opened = await new Promise((r) => ws.addEventListener('open', () => r(true)));
    check('the live connection opens', !!opened, '');
    const until = Date.now() + 10000;
    while (!frames.some((f) => f.t === 'snapshot') && Date.now() < until) await sleep(50);
    const snap = frames.find((f) => f.t === 'snapshot');
    const spaces = snap && snap.state && snap.state.spaces || {};
    const space = spaces[String(spaceId)];
    check('the snapshot space carries the emoji set', space && Object.keys(space.emoji || {}).length === 1, JSON.stringify(space ? Object.keys(space.emoji || {}) : 'no space'));
    check('the snapshot space carries the sticker set', space && (Object.entries(space.stickers || {})[0] || [])[1] && (Object.entries(space.stickers || {})[0] || [])[1].name === 'fire', JSON.stringify((Object.entries(space.stickers || {}))[0] || 'none'));

    await call('rename_emoji', [spaceId, emojiId, 'smile'], admin.jar);
    await sleep(500);
    const cs2 = await call('create_sticker', [spaceId, 'wave', 'a wave', st.slot], admin.jar);
    check('the admin adds a second sticker', !!cs2.body.Ok, JSON.stringify(cs2.body.Err || {}));
    await sleep(700);
    const eF = frames.filter((f) => f.t === 'event' && f.event && f.event.e === 'emoji_changed');
    const sF = frames.filter((f) => f.t === 'event' && f.event && f.event.e === 'stickers_changed');
    check("the member's live events restate the emoji rename", eF.some((f) => (f.event.emoji || []).some((x) => x.name === 'smile')), `emoji_changed frames: ${eF.length}`);
    check("the member's live events restate the sticker set", sF.some((f) => (f.event.stickers || []).length === 2), `stickers_changed frames: ${sF.length}`);

    // A sticker-only message sends with an empty body.
    const channels = snap && snap.state && snap.state.channels || {};
    const general = Object.values(channels).find((c) => String(c.space) === String(spaceId) && c.kind === 'text');
    const sm = await call('send', [general && general.id, '', null, [], stickerId], member.jar);
    check('a sticker-only message sends', !!sm.body.Ok, JSON.stringify(sm.body.Err || {}));

    // A foreign sticker is refused.
    const other = await call('create_space', ['Elsewhere'], admin.jar);
    check('the admin creates a second space', !!other.body.Ok, JSON.stringify(other.body.Err || {}));
    if (other.body.Ok) {
      const oinv = await call('create_invite', [other.body.Ok, null, null], admin.jar);
      const oreg = oinv.body.Ok && await call('join_space', [oinv.body.Ok.code], member.jar);
      const ocs = oreg && oreg.body.Ok && await call('create_sticker', [other.body.Ok, 'alien', 'not ours', st.slot], admin.jar);
      if (ocs && ocs.body.Ok) {
        const badSend = await call('send', [general && general.id, '', null, [], ocs.body.Ok.id], member.jar);
        check("another space's sticker is refused at send", !badSend.body.Ok, JSON.stringify(badSend.body.Ok || {}));
        check('with the sentence about the space', (badSend.raw || '').includes("isn't one of this space's"), JSON.stringify(badSend.body.Err || {}));
      }
    }

    // Delete reaches the member live and in the list.
    if (emojiId) {
      const del = await call('delete_emoji', [spaceId, emojiId], admin.jar);
      check('the admin deletes the emoji', 'Ok' in del.body, JSON.stringify(del.body.Err || {}));
      await sleep(700);
      const eF2 = frames.filter((f) => f.t === 'event' && f.event && f.event.e === 'emoji_changed');
      check('the member sees the emoji set emptied live', eF2.some((f) => (f.event.emoji || []).length === 0), `last emoji_changed: ${JSON.stringify((eF2[eF2.length - 1] || {}).event || {})}`);
      const le2 = await call('list_emoji', [spaceId], member.jar);
      check('the member no longer lists it', !!le2.body.Ok && (le2.body.Ok || []).length === 0, JSON.stringify(le2.body.Err || {}));
    }
    ws.close();
  }

  cleanup(server);
  console.log(`FLOW RESULT: ${pass} PASS, ${fail} FAIL`);
}

function cleanup(server) {
  try { server.kill(); } catch {}
}

main().catch((e) => { console.log(`FLOW RESULT: ${pass} PASS, ${fail} FAIL (fatal: ${e.stack || e})`); });
