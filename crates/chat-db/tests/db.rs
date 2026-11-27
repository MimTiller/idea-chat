//! Every query, against a real Postgres. Skipped without
//! `CHAT_TEST_DATABASE_URL` (a server URL; each test makes its own database).

use chat_db::{CodeCheck, CodePurpose, Db, InviteOutcome, NewAccount, NewMessage, NewSpace};
use chat_model::perms::Permission;
use chat_model::{ChannelKind, Id, Level, MessageKind, NotificationSetting, Presence, Relation, Scope, Status, MUTED_UNTIL_TURNED_OFF};
use sqlx::Connection;

async fn db() -> Option<Db> {
    let Ok(base) = std::env::var("CHAT_TEST_DATABASE_URL") else {
        eprintln!("CHAT_TEST_DATABASE_URL not set; skipping");
        return None;
    };
    let name = format!("chat_test_{}", uuid::Uuid::now_v7().simple());
    let mut admin = sqlx::PgConnection::connect(&base).await.expect("connect to the test server");
    sqlx::query(&format!("CREATE DATABASE {name}")).execute(&mut admin).await.unwrap();
    let url = match base.rsplit_once('/') {
        Some((server, _)) => format!("{server}/{name}"),
        None => panic!("CHAT_TEST_DATABASE_URL must name a database"),
    };
    Some(Db::connect(&url).await.expect("migrate"))
}

async fn account(db: &Db, name: &str) -> Id {
    let a = db
        .create_account(&NewAccount {
            email: format!("{name}@example.com"),
            password: "correct horse battery".into(),
            handle: name.into(),
            display_name: name.to_uppercase(),
        })
        .await
        .unwrap();
    a.id()
}

fn text(channel: Id, author: Id, body: &str) -> NewMessage {
    NewMessage {
        channel,
        author,
        kind: MessageKind::Text,
        body: body.into(),
        reply_to: None,
        mentions: vec![],
        everyone: false,
        mention_roles: vec![],
        attachments: vec![],
        sticker: None,
    }
}

#[tokio::test]
async fn accounts_are_unique_by_email_and_handle_case_insensitively() {
    let Some(db) = db().await else { return };
    account(&db, "ana").await;
    let dup = db
        .create_account(&NewAccount {
            email: "ANA@example.com".into(),
            password: "x".repeat(12),
            handle: "other".into(),
            display_name: "x".into(),
        })
        .await;
    assert!(matches!(dup, Err(chat_db::Error::Taken("That email"))));
    let dup = db
        .create_account(&NewAccount {
            email: "new@example.com".into(),
            password: "x".repeat(12),
            handle: "ANA".into(),
            display_name: "x".into(),
        })
        .await;
    assert!(matches!(dup, Err(chat_db::Error::Taken("That username"))));
}

#[tokio::test]
async fn passwords_check_and_changing_one_bumps_the_epoch() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let a = db.account_by_email("Ana@Example.com").await.unwrap().unwrap();
    assert!(a.check_password("correct horse battery"));
    assert!(!a.check_password("wrong"));
    assert_eq!(a.epoch, 0);
    assert_eq!(db.set_password(ana, "a new long password").await.unwrap(), 1);
    assert!(db.account(ana).await.unwrap().check_password("a new long password"));
}

#[tokio::test]
async fn email_codes_are_single_use_and_lock_after_too_many_guesses() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let code = db.issue_code(ana, CodePurpose::Verify, time::Duration::minutes(15)).await.unwrap();
    assert_eq!(code.len(), 6);
    let wrong = if code == "000000" { "111111" } else { "000000" };
    assert_eq!(db.check_code(ana, CodePurpose::Verify, wrong).await.unwrap(), CodeCheck::Wrong);
    assert_eq!(db.check_code(ana, CodePurpose::Verify, &code).await.unwrap(), CodeCheck::Ok);
    assert_eq!(db.check_code(ana, CodePurpose::Verify, &code).await.unwrap(), CodeCheck::Expired);

    let code = db.issue_code(ana, CodePurpose::Reset, time::Duration::minutes(15)).await.unwrap();
    let wrong = if code == "000000" { "111111" } else { "000000" };
    for _ in 0..4 {
        assert_eq!(db.check_code(ana, CodePurpose::Reset, wrong).await.unwrap(), CodeCheck::Wrong);
    }
    assert_eq!(db.check_code(ana, CodePurpose::Reset, wrong).await.unwrap(), CodeCheck::Locked);
    assert_eq!(db.check_code(ana, CodePurpose::Reset, &code).await.unwrap(), CodeCheck::Locked, "locked even for the right code");
}

#[tokio::test]
async fn a_new_space_has_an_owner_and_starter_channels() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let (space, channels) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    assert_eq!(space.owner, ana);
    assert_eq!(space.categories.len(), 2);
    assert!(channels.iter().any(|c| c.kind == ChannelKind::Text && c.name == "general"));
    assert!(channels.iter().any(|c| c.kind == ChannelKind::Voice));
    assert_eq!(db.spaces_of(ana).await.unwrap(), vec![space.id]);
}

#[tokio::test]
async fn a_new_space_seeds_admin_and_moderator_roles_that_nobody_holds() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let (space, _) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    let admin = space.roles.values().into_iter().find(|r| r.name == "Admin").unwrap();
    let moderator = space.roles.values().into_iter().find(|r| r.name == "Moderator").unwrap();
    assert_eq!(space.roles.len(), 2, "exactly the seeded roles");
    assert_eq!(admin.permissions, chat_model::perms::ADMIN, "Admin is the tag alone");
    assert_eq!(admin.position, 0);
    assert_eq!(moderator.permissions, chat_model::perms::DEFAULT_MODERATOR);
    assert_eq!(moderator.position, 1);
    assert_ne!(admin.id, space.id);
    assert_ne!(moderator.id, space.id);
    assert_eq!(admin.colour, Some(0xE74C3C), "the seeded Admin is red");
    assert_eq!(moderator.colour, Some(0x2ECC71), "the seeded Moderator is green");
    assert!(!admin.mentionable, "the seeded Admin answers no role ping");
    assert!(moderator.mentionable, "the seeded Moderator answers a role ping");
    assert_eq!(db.role_ids(space.id, ana).await.unwrap(), Some(Vec::<Id>::new()), "the owner holds neither");
    let role = db.create_role(space.id, "Bot", Permission::AddReaction.bit(), None, false).await.unwrap();
    assert_eq!(role.position, 2, "a hand-made role still lands at the bottom of the seeded ladder");
}

/// Two people racing for the last use of an invite: exactly one gets in.
#[tokio::test]
async fn the_last_use_of_an_invite_goes_to_one_person() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let (bob, cat) = (account(&db, "bob").await, account(&db, "cat").await);
    let (space, _) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    let invite = db.create_invite(space.id, ana, Some(1), None).await.unwrap();
    let (a, b) = tokio::join!(db.redeem_invite(&invite.code, bob), db.redeem_invite(&invite.code, cat));
    let joined = [a, b].into_iter().filter(|r| matches!(r, Ok(InviteOutcome::Joined { .. }))).count();
    assert_eq!(joined, 1);
    assert!(db.invite_preview(&invite.code, None).await.is_err(), "used up");
}

#[tokio::test]
async fn redeeming_twice_uses_nothing_up() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let bob = account(&db, "bob").await;
    let (space, _) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    let invite = db.create_invite(space.id, ana, Some(2), None).await.unwrap();
    assert!(matches!(db.redeem_invite(&invite.code, bob).await.unwrap(), InviteOutcome::Joined { .. }));
    assert!(matches!(db.redeem_invite(&invite.code, bob).await.unwrap(), InviteOutcome::AlreadyMember { .. }));
    assert_eq!(db.invites(space.id).await.unwrap()[0].uses, 1);
}

#[tokio::test]
async fn the_owner_cannot_leave_but_can_transfer() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let bob = account(&db, "bob").await;
    let (space, _) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    let invite = db.create_invite(space.id, ana, None, None).await.unwrap();
    db.redeem_invite(&invite.code, bob).await.unwrap();
    assert!(db.remove_member(space.id, ana).await.is_err());
    db.transfer_space(space.id, ana, bob).await.unwrap();
    assert_eq!(db.role_ids(space.id, bob).await.unwrap(), Some(Vec::<Id>::new()));
    db.remove_member(space.id, ana).await.unwrap();
    assert_eq!(db.space(space.id).await.unwrap().owner, bob);
}

#[tokio::test]
async fn roles_are_created_named_and_deleted_with_their_holders() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let (space, _) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    let bob = account(&db, "bob").await;
    let invite = db.create_invite(space.id, ana, None, None).await.unwrap();
    db.redeem_invite(&invite.code, bob).await.unwrap();

    let role = db.create_role(space.id, "Mod", Permission::ManageMembers.bit(), Some(0x3498DB), true).await.unwrap();
    assert_eq!(role.colour, Some(0x3498DB), "the colour the caller sends is the colour stored");
    assert!(role.mentionable, "the mentionable flag the caller sends is stored");
    db.set_role_ids(space.id, bob, &[role.id]).await.unwrap();
    assert_eq!(db.role_ids(space.id, bob).await.unwrap(), Some(vec![role.id]));

    db.update_role(space.id, role.id, "Moderator", Permission::ManageMembers.bit() | Permission::ManagePins.bit(), Some(0x9B59B6), false).await.unwrap();
    let after_update = db.space(space.id).await.unwrap();
    let updated = after_update.roles.get(&role.id).unwrap();
    assert_eq!(updated.name, "Moderator");
    assert_eq!(updated.permissions, Permission::ManageMembers.bit() | Permission::ManagePins.bit());
    assert_eq!(updated.colour, Some(0x9B59B6), "an update restates the colour exactly as sent");
    assert!(!updated.mentionable, "an update restates mentionable too");
    db.update_role(space.id, role.id, "Mod Two", Permission::ManageMembers.bit(), None, true).await.unwrap();
    assert_eq!(db.space(space.id).await.unwrap().roles.get(&role.id).unwrap().colour, None, "None clears it");

    db.delete_role(space.id, role.id).await.unwrap();
    let after = db.space(space.id).await.unwrap();
    assert_eq!(db.role_ids(space.id, bob).await.unwrap(), Some(Vec::<Id>::new()));
    assert_eq!(after.roles.len(), 2, "only the seeded roles are left");
}

#[tokio::test]
async fn posting_counts_mentions_and_reading_clears_them() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let bob = account(&db, "bob").await;
    let (space, channels) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    let invite = db.create_invite(space.id, ana, None, None).await.unwrap();
    db.redeem_invite(&invite.code, bob).await.unwrap();
    let general = channels.iter().find(|c| c.kind == ChannelKind::Text).unwrap().id;

    db.post(&text(general, ana, "no mention")).await.unwrap();
    let posted = db.post(&NewMessage { mentions: vec![bob], ..text(general, ana, "hey bob") }).await.unwrap();
    assert_eq!(posted.notified, vec![bob]);
    let everyone = db.post(&NewMessage { everyone: true, ..text(general, ana, "all") }).await.unwrap();
    assert_eq!(everyone.notified, vec![bob]);

    let watcher = db.create_role(space.id, "Watcher", Permission::ViewChannel.bit(), None, true).await.unwrap();
    db.set_role_ids(space.id, bob, &[watcher.id]).await.unwrap();
    let roled = db.post(&NewMessage { mention_roles: vec![watcher.id], ..text(general, ana, "ping") }).await.unwrap();
    assert_eq!(roled.notified, vec![bob], "a role mention reaches the holders");

    let reads = db.reads_of(bob).await.unwrap();
    assert_eq!(reads[&general].mentions, 3);
    assert_eq!(db.reads_of(ana).await.unwrap()[&general].last_read, Some(roled.message.id));
    assert_eq!(db.channel(general).await.unwrap().last_message, Some(roled.message.id));

    db.mark_read(bob, general, roled.message.id).await.unwrap();
    assert_eq!(db.reads_of(bob).await.unwrap()[&general].mentions, 0);
    // Never backwards.
    let back = db.mark_read(bob, general, posted.message.id).await.unwrap();
    assert_eq!(back, roled.message.id);
}

#[tokio::test]
async fn history_pages_both_ways_and_says_where_it_ends() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let (_, channels) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    let general = channels.iter().find(|c| c.kind == ChannelKind::Text).unwrap().id;
    let mut ids = vec![];
    for i in 0..7 {
        ids.push(db.post(&text(general, ana, &format!("m{i}"))).await.unwrap().message.id);
    }
    let newest = db.page(general, None, None, 3).await.unwrap();
    assert_eq!(newest.messages.iter().map(|m| m.id).collect::<Vec<_>>(), ids[4..].to_vec());
    assert!(newest.older && newest.present);
    let older = db.page(general, Some(ids[4]), None, 3).await.unwrap();
    assert_eq!(older.messages.iter().map(|m| m.id).collect::<Vec<_>>(), ids[1..4].to_vec());
    assert!(older.older && !older.present);
    let oldest = db.page(general, Some(ids[1]), None, 3).await.unwrap();
    assert!(!oldest.older);
    let forward = db.page(general, None, Some(ids[1]), 10).await.unwrap();
    assert_eq!(forward.messages.len(), 5);
    assert!(forward.present);
    let around = db.around(general, ids[3], 1).await.unwrap();
    assert_eq!(around.messages.iter().map(|m| m.id).collect::<Vec<_>>(), ids[2..5].to_vec());
}

#[tokio::test]
async fn edits_reactions_replies_and_deletes() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let (_, channels) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    let general = channels.iter().find(|c| c.kind == ChannelKind::Text).unwrap().id;
    let first = db.post(&text(general, ana, "first")).await.unwrap().message;
    let reply = db.post(&NewMessage { reply_to: Some(first.id), ..text(general, ana, "re") }).await.unwrap().message;
    assert_eq!(reply.reply_to.as_ref().unwrap().excerpt, "first");

    let edited = db.edit_message(first.id, "first!", &[], false, &[]).await.unwrap();
    assert!(edited.edited_at.is_some());
    assert!(db.react(first.id, ana, "👍", true).await.unwrap());
    assert!(!db.react(first.id, ana, "👍", true).await.unwrap());
    assert_eq!(db.message(first.id).await.unwrap().reactions[0].users, vec![ana]);

    db.delete_message(reply.id).await.unwrap();
    assert_eq!(db.channel(general).await.unwrap().last_message, Some(first.id), "falls back to what is newest");
    db.delete_message(first.id).await.unwrap();
    assert_eq!(db.channel(general).await.unwrap().last_message, None);
}

#[tokio::test]
async fn search_finds_words_in_the_given_channels() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let (_, channels) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    let general = channels.iter().find(|c| c.kind == ChannelKind::Text).unwrap().id;
    db.post(&text(general, ana, "the quarterly roadmap is ready")).await.unwrap();
    db.post(&text(general, ana, "lunch?")).await.unwrap();
    let hits = db.search(&[general], "roadmap", None, 10).await.unwrap();
    assert_eq!(hits.len(), 1);
    assert!(db.search(&[], "roadmap", None, 10).await.unwrap().is_empty());
}

#[tokio::test]
async fn threads_grow_from_one_message_each() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let (_, channels) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    let general = channels.iter().find(|c| c.kind == ChannelKind::Text).unwrap().clone();
    let starter = db.post(&text(general.id, ana, "idea")).await.unwrap().message;
    let thread = db.create_thread(&general, "the idea", Some(starter.id), ana).await.unwrap();
    assert_eq!(thread.parent, Some(general.id));
    assert!(db.create_thread(&general, "again", Some(starter.id), ana).await.is_err());
    db.post(&text(thread.id, ana, "in thread")).await.unwrap();
    let link = db.message(starter.id).await.unwrap().thread.unwrap();
    assert_eq!((link.id, link.message_count), (thread.id, 1));
}

#[tokio::test]
async fn one_direct_conversation_per_pair_hidden_from_the_other_until_used() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let bob = account(&db, "bob").await;
    let (dm, created) = db.open_direct(ana, bob).await.unwrap();
    assert!(created);
    let (again, created) = db.open_direct(bob, ana).await.unwrap();
    assert!(!created);
    assert_eq!(dm.id, again.id);
    assert_eq!(db.conversations_of(ana).await.unwrap().len(), 1);
    db.hide_conversation(dm.id, bob).await.unwrap();
    assert!(db.conversations_of(bob).await.unwrap().is_empty());
    let posted = db.post(&text(dm.id, ana, "hi")).await.unwrap();
    assert_eq!(posted.notified, vec![bob], "every direct message notifies");
    assert_eq!(db.unhide_conversation(dm.id).await.unwrap(), vec![bob]);
    assert_eq!(db.conversations_of(bob).await.unwrap().len(), 1);
}

#[tokio::test]
async fn friend_requests_accept_when_both_ask_and_blocks_are_one_sided() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let bob = account(&db, "bob").await;
    let sent = db.befriend(ana, bob).await.unwrap();
    assert_eq!((sent.mine, sent.theirs), (Some(Relation::Outgoing), Some(Relation::Incoming)));
    let accepted = db.befriend(bob, ana).await.unwrap();
    assert_eq!(accepted.mine, Some(Relation::Friend));
    assert_eq!(db.relation(ana, bob).await.unwrap(), Some(Relation::Friend));

    db.block(bob, ana).await.unwrap();
    assert_eq!(db.relation(ana, bob).await.unwrap(), None);
    assert!(db.blocked_between(ana, bob).await.unwrap());
    // Asking someone who blocked you looks sent, and changes nothing for them.
    let ask = db.befriend(ana, bob).await.unwrap();
    assert_eq!(ask.mine, Some(Relation::Outgoing));
    assert_eq!(db.relation(bob, ana).await.unwrap(), Some(Relation::Blocked));
}

#[tokio::test]
async fn presence_follows_connections_and_chosen_status() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    assert!(db.presence(&[ana]).await.unwrap().is_empty());
    let conn = chat_db::new_id();
    db.open_connection(conn, ana, None, "test").await.unwrap();
    assert_eq!(db.presence_one(ana).await.unwrap().presence, Presence::Online);
    db.set_idle(conn, true).await.unwrap();
    assert_eq!(db.presence_one(ana).await.unwrap().presence, Presence::Idle);
    db.set_status(ana, &Status { presence: Presence::Offline, text: None }).await.unwrap();
    assert!(db.presence(&[ana]).await.unwrap().is_empty(), "appearing offline");
    db.disconnect(conn).await.unwrap();
}

#[tokio::test]
async fn the_first_into_a_call_keys_it_and_the_last_out_ends_it() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let bob = account(&db, "bob").await;
    let (space, channels) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    let invite = db.create_invite(space.id, ana, None, None).await.unwrap();
    db.redeem_invite(&invite.code, bob).await.unwrap();
    let voice = channels.iter().find(|c| c.kind == ChannelKind::Voice).unwrap().id;
    let (ca, cb) = (chat_db::new_id(), chat_db::new_id());
    db.open_connection(ca, ana, None, "t").await.unwrap();
    db.open_connection(cb, bob, None, "t").await.unwrap();
    let da = db.register_device(ana, "laptop", "AAAA").await.unwrap();
    let dbb = db.register_device(bob, "phone", "BBBB").await.unwrap();

    let a = db.join_call(ana, voice, ca, da.id).await.unwrap();
    assert!(a.make_key && a.started);
    let b = db.join_call(bob, voice, cb, dbb.id).await.unwrap();
    assert!(!b.make_key && !b.started);
    assert_eq!(a.room, b.room);
    assert_eq!(db.room_devices(a.room).await.unwrap().len(), 2);

    let left = db.leave_call(ana).await.unwrap().unwrap();
    assert!(!left.ended);
    // Dropping the connection takes its person out of the call.
    let left = db.disconnect(cb).await.unwrap().unwrap();
    assert!(left.ended);
    let again = db.join_call(ana, voice, ca, da.id).await.unwrap();
    assert!(again.started && again.make_key);
    assert_ne!(again.room, a.room, "a new instance has a new room, and so a new key");
}

#[tokio::test]
async fn the_roles_migration_backfills_the_old_ranks() {
    let Ok(base) = std::env::var("CHAT_TEST_DATABASE_URL") else {
        eprintln!("CHAT_TEST_DATABASE_URL not set; skipping");
        return;
    };
    // A database seeded with the old schema: 0001 on its own, data in the
    // old shape (an admin-ranked member and a channel locked to admins),
    // then the roles migration. The backfill is what carries that data
    // across; without it the admin loses power and the lock opens.
    let name = format!("chat_test_{}", uuid::Uuid::now_v7().simple());
    {
        let mut admin = sqlx::PgConnection::connect(&base).await.expect("connect to the test server");
        sqlx::query(&format!("CREATE DATABASE {name}")).execute(&mut admin).await.unwrap();
    }
    let url = match base.rsplit_once('/') {
        Some((server, _)) => format!("{server}/{name}"),
        None => panic!("CHAT_TEST_DATABASE_URL must name a database"),
    };
    let migrations = sqlx::migrate!("./migrations");
    let sql_of = |version| migrations.iter()
        .find(|m| m.version == version)
        .unwrap()
        .sql
        .to_string();
    let mut conn = sqlx::PgConnection::connect(&url).await.expect("connect");
    sqlx::raw_sql(&sql_of(1)).execute(&mut conn).await.unwrap();
    let seed = "INSERT INTO users (id, email, password_hash, handle, display_name) VALUES ('00000000-0000-0000-0000-000000000001', 'a@x', 'x', 'a', 'A'), ('00000000-0000-0000-0000-000000000002', 'b@x', 'x', 'b', 'B'); INSERT INTO spaces (id, name, owner_id) VALUES ('00000000-0000-0000-0000-000000000009', 'sp', '00000000-0000-0000-0000-000000000001'); INSERT INTO space_members (space_id, user_id, role) VALUES ('00000000-0000-0000-0000-000000000009', '00000000-0000-0000-0000-000000000001', 'owner'), ('00000000-0000-0000-0000-000000000009', '00000000-0000-0000-0000-000000000002', 'admin'); INSERT INTO channels (id, kind, space_id, name, created_by, view_role, post_role) VALUES ('00000000-0000-0000-0000-0000000000a0', 'text', '00000000-0000-0000-0000-000000000009', 'ann', '00000000-0000-0000-0000-000000000001', 'admin', 'admin'), ('00000000-0000-0000-0000-0000000000a1', 'text', '00000000-0000-0000-0000-000000000009', 'open', '00000000-0000-0000-0000-000000000001', 'member', 'member');";
    sqlx::raw_sql(seed).execute(&mut conn).await.unwrap();
    sqlx::raw_sql(&sql_of(2)).execute(&mut conn).await.unwrap();

    let admins: i64 = sqlx::query_scalar("SELECT count(*) FROM space_roles WHERE name = 'Admin'")
        .fetch_one(&mut conn)
        .await.unwrap();
    assert_eq!(admins, 1i64, "the space with old admins gets an Admin role");
    let held: i64 = sqlx::query_scalar("SELECT count(*) FROM space_members WHERE role_ids <> '{}'")
        .fetch_one(&mut conn)
        .await.unwrap();
    assert_eq!(held, 1i64, "exactly the old admin holds the backfilled role");
    let locked: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM channels WHERE name = 'ann' AND view_mask <> 0 AND post_mask <> 0",
    )
        .fetch_one(&mut conn)
        .await.unwrap();
    assert_eq!(locked, 1i64, "the admin-locked channel keeps its lock as masks");
    let open: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM channels WHERE name = 'open' AND view_mask = 0 AND post_mask = 0",
    )
        .fetch_one(&mut conn)
        .await.unwrap();
    assert_eq!(open, 1i64, "the open channel stays open");
}

#[tokio::test]
async fn the_snapshot_holds_spaces_conversations_and_people() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let bob = account(&db, "bob").await;
    let (space, _) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    db.open_direct(ana, bob).await.unwrap();
    db.befriend(ana, bob).await.unwrap();
    let s = db.snapshot(ana).await.unwrap();
    assert_eq!(s.me_id(), Some(ana));
    assert_eq!(s.space_order, vec![space.id]);
    assert_eq!(s.conversations().len(), 1);
    assert!(s.users.contains_key(&bob));
    assert_eq!(s.relations[&bob], Relation::Outgoing);
    assert!(!s.sidebar(space.id).is_empty());
}

#[tokio::test]
async fn emoji_and_stickers_are_made_renamed_counted_and_removed() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let (space, _) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();

    let wave_upload = db.create_upload(ana, "emoji", "wave.png", 1024, "image/png").await.unwrap();
    db.confirm_upload(wave_upload.id, 1024, Some((16, 16))).await.unwrap();
    let wave = db.create_emoji(space.id, "wave", wave_upload.id, ana).await.unwrap();
    assert_eq!(db.emoji_count(space.id).await.unwrap(), 1);

    db.rename_emoji(space.id, wave.id, "hello").await.unwrap();
    let after_rename = db.space(space.id).await.unwrap();
    assert_eq!(after_rename.emoji[&wave.id].name, "hello");
    assert_eq!(after_rename.emoji[&wave.id].upload, wave_upload.id);

    let clash = db.create_emoji(space.id, "hello", wave_upload.id, ana).await;
    assert!(matches!(clash, Err(chat_db::Error::Taken("That emoji name"))));

    let stick_upload = db.create_upload(ana, "sticker", "cat.png", 2048, "image/png").await.unwrap();
    db.confirm_upload(stick_upload.id, 2048, Some((64, 64))).await.unwrap();
    let cat = db.create_sticker(space.id, "cat", "a cat", stick_upload.id, ana).await.unwrap();
    assert_eq!(db.sticker_count(space.id).await.unwrap(), 1);
    assert_eq!(db.space(space.id).await.unwrap().stickers[&cat.id].description, "a cat");

    // A message posts without a sticker, and reads back with none.
    let general = db.space_channels(space.id).await.unwrap()
        .iter().find(|c| c.kind == ChannelKind::Text).unwrap().id;
    let posted = db.post(&text(general, ana, "hi")).await.unwrap();
    assert_eq!(posted.message.sticker, None);

    // A sticker-only message: empty body, the space's sticker id carried in.
    let starred = db.post(&NewMessage { sticker: Some(cat.id), ..text(general, ana, "") }).await.unwrap();
    assert_eq!(starred.message.sticker, Some(cat.id));
    assert_eq!(db.message(starred.message.id).await.unwrap().body, "");

    // The emoji and sticker uploads are claimed by the space, so the
    // unclaimed-message sweep never reaches them.
    assert_eq!(db.sweep_uploads().await.unwrap(), Vec::<String>::new());
    let still_there = db.upload(wave_upload.id, ana).await.unwrap();
    assert_eq!(still_there.id, wave_upload.id);

    db.delete_emoji(space.id, wave.id).await.unwrap();
    db.delete_sticker(space.id, cat.id).await.unwrap();
    let after = db.space(space.id).await.unwrap();
    assert!(after.emoji.is_empty() && after.stickers.is_empty());
    assert_eq!(db.emoji_count(space.id).await.unwrap(), 0);
    assert_eq!(db.sticker_count(space.id).await.unwrap(), 0);
    // The old message still reads back, with its sticker dropped.
    assert_eq!(db.message(posted.message.id).await.unwrap().body, "hi");
}

#[tokio::test]
async fn notification_settings_store_replace_and_clear() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let bob = account(&db, "bob").await;
    let (space, channels) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    let general = channels.iter().find(|c| c.name == "general").unwrap();

    let space_row = NotificationSetting { scope: Scope::Space, target: space.id, level: Some(Level::Mentions), muted_until: None };
    let channel_row = NotificationSetting { scope: Scope::Channel, target: general.id, level: None, muted_until: Some(MUTED_UNTIL_TURNED_OFF) };
    db.set_notification_setting(ana, &space_row).await.unwrap();
    db.set_notification_setting(ana, &channel_row).await.unwrap();
    assert_eq!(db.notification_settings(ana, Some(space.id)).await.unwrap(), vec![channel_row.clone(), space_row.clone()], "channel rows sort before space rows");
    assert_eq!(db.notification_settings(ana, None).await.unwrap().len(), 2);
    assert!(db.notification_settings(bob, None).await.unwrap().is_empty(), "settings are per person");

    // Replacing, then clearing with an empty setting.
    let replaced = NotificationSetting { scope: Scope::Channel, target: general.id, level: Some(Level::Nothing), muted_until: None };
    db.set_notification_setting(ana, &replaced).await.unwrap();
    let cleared = NotificationSetting { scope: Scope::Space, target: space.id, level: None, muted_until: None };
    db.set_notification_setting(ana, &cleared).await.unwrap();
    assert_eq!(db.notification_settings(ana, Some(space.id)).await.unwrap(), vec![replaced]);

    // The snapshot carries them, so a connection sees its own settings.
    let s = db.snapshot(ana).await.unwrap();
    assert_eq!(s.settings.get(&general.id), Some(&replaced));
    assert!(s.settings.get(&space.id).is_none(), "the cleared row is gone");
    // The plain mute keeps working beside them.
    db.set_muted(ana, space.id, true).await.unwrap();
    assert_eq!(db.mutes_of(ana).await.unwrap(), vec![space.id]);
}

#[tokio::test]
async fn pins_are_pinned_listed_counted_and_removed() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let (_, channels) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    let general = channels.iter().find(|c| c.kind == ChannelKind::Text).unwrap().id;
    let first = db.post(&text(general, ana, "one")).await.unwrap().message.id;
    let second = db.post(&text(general, ana, "two")).await.unwrap().message.id;

    assert!(db.pin(first, ana).await.unwrap(), "the first pin is new");
    assert!(db.pin(second, ana).await.unwrap());
    assert!(!db.pin(first, ana).await.unwrap(), "pinning twice changes nothing");
    assert_eq!(db.pin_count(general).await.unwrap(), 2);
    // Newest pin first: second was pinned after first.
    assert_eq!(db.pins_of(general).await.unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), vec![second, first]);

    assert!(db.unpin(first).await.unwrap());
    assert!(!db.unpin(first).await.unwrap(), "unpinning twice changes nothing");
    assert_eq!(db.pin_count(general).await.unwrap(), 1);
    assert_eq!(db.pins_of(general).await.unwrap().iter().map(|m| m.id).collect::<Vec<_>>(), vec![second]);

    // Deleting the pinned message takes its pin with it.
    db.delete_message(second).await.unwrap();
    assert_eq!(db.pin_count(general).await.unwrap(), 0);
    assert!(db.pins_of(general).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_new_user_reads_back_with_no_cosmetics() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let u = db.user(ana).await.unwrap();
    assert_eq!(u.avatar_decoration, None);
    assert_eq!(u.profile_effect, None);
}

#[tokio::test]
async fn cosmetics_are_set_kept_cleared_and_seen_by_others() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let bob = account(&db, "bob").await;

    // A set returns the user with it.
    let u = db.set_cosmetics(ana, Some(Some("sparkle")), None).await.unwrap();
    assert_eq!(u.avatar_decoration, Some("sparkle".into()));
    assert_eq!(u.profile_effect, None);

    // Setting only the effect keeps the decoration.
    let u = db.set_cosmetics(ana, None, Some(Some("glow"))).await.unwrap();
    assert_eq!(u.avatar_decoration, Some("sparkle".into()));
    assert_eq!(u.profile_effect, Some("glow".into()));

    // Some(None) clears one field and leaves the other.
    let u = db.set_cosmetics(ana, Some(None), None).await.unwrap();
    assert_eq!(u.avatar_decoration, None);
    assert_eq!(u.profile_effect, Some("glow".into()));

    // A second person reading Ana through the same read sees it.
    db.set_cosmetics(ana, Some(Some("wave_2")), None).await.unwrap();
    let seen = db.user_by_handle("bob").await.unwrap().unwrap();
    assert_eq!(seen.id, bob, "the reader is a different account");
    let read = db.user_by_handle("ana").await.unwrap().unwrap();
    assert_eq!(read.avatar_decoration, Some("wave_2".into()));
    assert_eq!(read.profile_effect, Some("glow".into()));
}
