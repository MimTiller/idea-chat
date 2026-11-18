//! Every query, against a real Postgres. Skipped without
//! `CHAT_TEST_DATABASE_URL` (a server URL; each test makes its own database).

use chat_db::{CodeCheck, CodePurpose, Db, InviteOutcome, NewAccount, NewMessage, NewSpace};
use chat_model::perms::Permission;
use chat_model::{ChannelKind, Id, MessageKind, Presence, Relation, Status};
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
        attachments: vec![],
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
    assert_eq!(db.role_ids(space.id, ana).await.unwrap(), Some(Vec::<Id>::new()), "the owner holds neither");
    let role = db.create_role(space.id, "Bot", Permission::AddReaction.bit(), None).await.unwrap();
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

    let role = db.create_role(space.id, "Mod", Permission::ManageMembers.bit(), Some(0x3498DB)).await.unwrap();
    assert_eq!(role.colour, Some(0x3498DB), "the colour the caller sends is the colour stored");
    db.set_role_ids(space.id, bob, &[role.id]).await.unwrap();
    assert_eq!(db.role_ids(space.id, bob).await.unwrap(), Some(vec![role.id]));

    db.update_role(space.id, role.id, "Moderator", Permission::ManageMembers.bit() | Permission::ManagePins.bit(), Some(0x9B59B6)).await.unwrap();
    let after_update = db.space(space.id).await.unwrap();
    let updated = after_update.roles.get(&role.id).unwrap();
    assert_eq!(updated.name, "Moderator");
    assert_eq!(updated.permissions, Permission::ManageMembers.bit() | Permission::ManagePins.bit());
    assert_eq!(updated.colour, Some(0x9B59B6), "an update restates the colour exactly as sent");
    db.update_role(space.id, role.id, "Mod Two", Permission::ManageMembers.bit(), None).await.unwrap();
    assert_eq!(db.space(space.id).await.unwrap().roles.get(&role.id).unwrap().colour, None, "None clears it");

    db.delete_role(space.id, role.id).await.unwrap();
    let after = db.space(space.id).await.unwrap();
    assert_eq!(db.role_ids(space.id, bob).await.unwrap(), Some(Vec::<Id>::new()));
    assert_eq!(after.roles.len(), 2, "only the seeded roles are left");
}

#[tokio::test]
async fn a_timeout_holds_a_member_back_until_it_passes_or_is_lifted() {
    let Some(db) = db().await else { return };
    let ana = account(&db, "ana").await;
    let (space, _) = db.create_space(&NewSpace { name: "Club".into(), owner: ana }).await.unwrap();
    let bob = account(&db, "bob").await;
    let invite = db.create_invite(space.id, ana, None, None).await.unwrap();
    db.redeem_invite(&invite.code, bob).await.unwrap();

    assert!(!db.timed_out(space.id, bob).await.unwrap(), "nobody starts timed out");
    db.set_timeout(space.id, bob, Some(5)).await.unwrap();
    assert!(db.timed_out(space.id, bob).await.unwrap(), "five minutes from now is still ahead");
    assert!(!db.timed_out(space.id, ana).await.unwrap(), "and it is only bob's");
    db.set_timeout(space.id, bob, None).await.unwrap();
    assert!(!db.timed_out(space.id, bob).await.unwrap(), "clearing it lifts it");
    db.set_timeout(space.id, bob, Some(0)).await.unwrap();
    assert!(!db.timed_out(space.id, bob).await.unwrap(), "a zero-minute timeout has already passed");
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

    let reads = db.reads_of(bob).await.unwrap();
    assert_eq!(reads[&general].mentions, 2);
    assert_eq!(db.reads_of(ana).await.unwrap()[&general].last_read, Some(everyone.message.id));
    assert_eq!(db.channel(general).await.unwrap().last_message, Some(everyone.message.id));

    db.mark_read(bob, general, everyone.message.id).await.unwrap();
    assert_eq!(db.reads_of(bob).await.unwrap()[&general].mentions, 0);
    // Never backwards.
    let back = db.mark_read(bob, general, posted.message.id).await.unwrap();
    assert_eq!(back, everyone.message.id);
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

    let edited = db.edit_message(first.id, "first!", &[], false).await.unwrap();
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