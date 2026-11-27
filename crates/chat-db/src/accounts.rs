//! Accounts: sign-up, passwords, email codes, profiles and devices.

use argon2::password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;
use chat_model::{Device, Id, Me, Status, User};
use rand::Rng;
use sha2::{Digest, Sha256};
use time::{Duration, OffsetDateTime};

use crate::{millis, new_id, presence_of, presence_text, taken, Db, Error, Result};

/// What sign-up asks for, already validated by `chat_model::text`.
#[derive(Debug, Clone)]
pub struct NewAccount {
    pub email: String,
    pub password: String,
    pub handle: String,
    pub display_name: String,
}

/// An account as the API's auth code needs it.
#[derive(Debug, Clone)]
pub struct Account {
    pub me: Me,
    pub verified: bool,
    pub epoch: i32,
    password_hash: String,
}

impl Account {
    pub fn id(&self) -> Id {
        self.me.user.id
    }

    /// Whether `password` is this account's. Argon2id; slow on purpose.
    pub fn check_password(&self, password: &str) -> bool {
        check_hash(&self.password_hash, password)
    }
}

/// Spend about as long as a real password check, for an email that has no
/// account — so the time sign-in takes does not say which emails exist.
pub fn dummy_password_check(password: &str) {
    static DUMMY: std::sync::OnceLock<String> = std::sync::OnceLock::new();
    let hash = DUMMY.get_or_init(|| hash_password("not-a-real-password-anyone-has").expect("hashing a constant"));
    let _ = check_hash(hash, password);
}

fn hash_password(password: &str) -> Result<String> {
    let salt = SaltString::generate(&mut OsRng);
    Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| Error::Refused(format!("could not hash the password: {e}")))
}

fn check_hash(hash: &str, password: &str) -> bool {
    PasswordHash::new(hash).is_ok_and(|h| Argon2::default().verify_password(password.as_bytes(), &h).is_ok())
}

/// What an emailed code is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodePurpose {
    Verify,
    Reset,
}

impl CodePurpose {
    fn text(self) -> &'static str {
        match self {
            CodePurpose::Verify => "verify",
            CodePurpose::Reset => "reset",
        }
    }
}

/// How checking a code went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeCheck {
    Ok,
    Wrong,
    /// Expired, used, or never issued.
    Expired,
    /// Too many wrong guesses; a new code must be requested.
    Locked,
}

/// How many wrong guesses a code survives. Six digits and five guesses is a
/// one-in-two-hundred-thousand chance per code.
pub const CODE_ATTEMPTS: i32 = 5;

fn code_hash(user: Id, code: &str) -> String {
    hex::encode(Sha256::digest(format!("{user}:{code}").as_bytes()))
}

#[derive(sqlx::FromRow)]
struct AccountRow {
    id: Id,
    email: String,
    password_hash: String,
    handle: String,
    display_name: String,
    avatar: Option<String>,
    bio: String,
    status_presence: String,
    status_text: Option<String>,
    email_verified_at: Option<OffsetDateTime>,
    auth_epoch: i32,
}

impl AccountRow {
    fn into_account(self) -> Account {
        Account {
            verified: self.email_verified_at.is_some(),
            epoch: self.auth_epoch,
            password_hash: self.password_hash,
            me: Me {
                user: User {
                    id: self.id,
                    handle: self.handle,
                    display_name: self.display_name,
                    avatar: self.avatar,
                    bio: self.bio,
                    avatar_decoration: None,
                    profile_effect: None,
                },
                email: self.email,
                status: Status { presence: presence_of(&self.status_presence), text: self.status_text },
            },
        }
    }
}

const ACCOUNT_COLS: &str = "id, email, password_hash, handle, display_name, avatar, bio, avatar_decoration, profile_effect, status_presence, \
                            status_text, email_verified_at, auth_epoch";

#[derive(sqlx::FromRow)]
pub(crate) struct UserRow {
    pub id: Id,
    pub handle: String,
    pub display_name: String,
    pub avatar: Option<String>,
    pub bio: String,
    pub avatar_decoration: Option<String>,
    pub profile_effect: Option<String>,
}

impl From<UserRow> for User {
    fn from(r: UserRow) -> User {
        User { id: r.id, handle: r.handle, display_name: r.display_name, avatar: r.avatar, bio: r.bio, avatar_decoration: r.avatar_decoration, profile_effect: r.profile_effect }
    }
}

pub(crate) const USER_COLS: &str = "id, handle, display_name, avatar, bio, avatar_decoration, profile_effect";

#[derive(sqlx::FromRow)]
struct DeviceRow {
    id: Id,
    name: String,
    public_key: String,
    created_at: OffsetDateTime,
    last_seen: OffsetDateTime,
}

impl From<DeviceRow> for Device {
    fn from(r: DeviceRow) -> Device {
        Device {
            id: r.id,
            name: r.name,
            public_key: r.public_key,
            created_at: millis(r.created_at),
            last_seen: millis(r.last_seen),
        }
    }
}

impl Db {
    /// Make an unverified account.
    pub async fn create_account(&self, new: &NewAccount) -> Result<Account> {
        let hash = hash_password(&new.password)?;
        let row: AccountRow = sqlx::query_as(&format!(
            "INSERT INTO users (id, email, password_hash, handle, display_name) VALUES ($1, $2, $3, $4, $5) \
             RETURNING {ACCOUNT_COLS}"
        ))
        .bind(new_id())
        .bind(&new.email)
        .bind(hash)
        .bind(&new.handle)
        .bind(&new.display_name)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| match taken(e, "users_email", "That email") {
            Error::Sql(e) => taken(e, "users_handle", "That username"),
            other => other,
        })?;
        Ok(row.into_account())
    }

    pub async fn account_by_email(&self, email: &str) -> Result<Option<Account>> {
        let row: Option<AccountRow> =
            sqlx::query_as(&format!("SELECT {ACCOUNT_COLS} FROM users WHERE lower(email) = lower($1)"))
                .bind(email)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(AccountRow::into_account))
    }

    pub async fn account(&self, id: Id) -> Result<Account> {
        let row: Option<AccountRow> = sqlx::query_as(&format!("SELECT {ACCOUNT_COLS} FROM users WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(AccountRow::into_account).ok_or(Error::NotFound)
    }

    /// Whether a username is free.
    pub async fn handle_free(&self, handle: &str) -> Result<bool> {
        let n: i64 = sqlx::query_scalar("SELECT count(*) FROM users WHERE lower(handle) = lower($1)")
            .bind(handle)
            .fetch_one(&self.pool)
            .await?;
        Ok(n == 0)
    }

    /// A new six-digit code for `user`, replacing any earlier one. The code
    /// is returned to be emailed and stored only hashed.
    pub async fn issue_code(&self, user: Id, purpose: CodePurpose, ttl: Duration) -> Result<String> {
        let code = format!("{:06}", rand::thread_rng().gen_range(0..1_000_000u32));
        sqlx::query(
            "INSERT INTO email_codes (user_id, purpose, code_hash, expires_at) VALUES ($1, $2, $3, $4) \
             ON CONFLICT (user_id, purpose) DO UPDATE \
             SET code_hash = EXCLUDED.code_hash, expires_at = EXCLUDED.expires_at, attempts = 0, created_at = now()",
        )
        .bind(user)
        .bind(purpose.text())
        .bind(code_hash(user, &code))
        .bind(OffsetDateTime::now_utc() + ttl)
        .execute(&self.pool)
        .await?;
        Ok(code)
    }

    /// When the current code for `user` was issued, for throttling resends.
    pub async fn code_issued_at(&self, user: Id, purpose: CodePurpose) -> Result<Option<OffsetDateTime>> {
        Ok(sqlx::query_scalar("SELECT created_at FROM email_codes WHERE user_id = $1 AND purpose = $2")
            .bind(user)
            .bind(purpose.text())
            .fetch_optional(&self.pool)
            .await?)
    }

    /// Check a code. A right one is used up; a wrong one costs an attempt.
    pub async fn check_code(&self, user: Id, purpose: CodePurpose, code: &str) -> Result<CodeCheck> {
        let mut tx = self.pool.begin().await?;
        let row: Option<(String, i32, OffsetDateTime)> = sqlx::query_as(
            "SELECT code_hash, attempts, expires_at FROM email_codes WHERE user_id = $1 AND purpose = $2 FOR UPDATE",
        )
        .bind(user)
        .bind(purpose.text())
        .fetch_optional(&mut *tx)
        .await?;
        let Some((hash, attempts, expires)) = row else { return Ok(CodeCheck::Expired) };
        if expires < OffsetDateTime::now_utc() {
            return Ok(CodeCheck::Expired);
        }
        if attempts >= CODE_ATTEMPTS {
            return Ok(CodeCheck::Locked);
        }
        let given = code_hash(user, code.trim());
        // Both are hex of a SHA-256; comparing them in constant time is
        // belt-and-braces, since the attempt limit already bounds guessing.
        let same = given.len() == hash.len() && given.bytes().zip(hash.bytes()).fold(0u8, |a, (x, y)| a | (x ^ y)) == 0;
        if same {
            sqlx::query("DELETE FROM email_codes WHERE user_id = $1 AND purpose = $2")
                .bind(user)
                .bind(purpose.text())
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            Ok(CodeCheck::Ok)
        } else {
            sqlx::query("UPDATE email_codes SET attempts = attempts + 1 WHERE user_id = $1 AND purpose = $2")
                .bind(user)
                .bind(purpose.text())
                .execute(&mut *tx)
                .await?;
            tx.commit().await?;
            Ok(if attempts + 1 >= CODE_ATTEMPTS { CodeCheck::Locked } else { CodeCheck::Wrong })
        }
    }

    pub async fn mark_verified(&self, user: Id) -> Result<()> {
        sqlx::query("UPDATE users SET email_verified_at = coalesce(email_verified_at, now()) WHERE id = $1")
            .bind(user)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    /// Change a password. Every session minted before is signed out.
    pub async fn set_password(&self, user: Id, password: &str) -> Result<i32> {
        let hash = hash_password(password)?;
        Ok(sqlx::query_scalar(
            "UPDATE users SET password_hash = $2, auth_epoch = auth_epoch + 1 WHERE id = $1 RETURNING auth_epoch",
        )
        .bind(user)
        .bind(hash)
        .fetch_one(&self.pool)
        .await?)
    }

    /// Sign out every session, and return the new epoch.
    pub async fn bump_epoch(&self, user: Id) -> Result<i32> {
        Ok(sqlx::query_scalar("UPDATE users SET auth_epoch = auth_epoch + 1 WHERE id = $1 RETURNING auth_epoch")
            .bind(user)
            .fetch_one(&self.pool)
            .await?)
    }

    /// Change what others see. Each field left `None` is kept; `avatar` is
    /// `Some(None)` to remove the picture.
    pub async fn update_profile(
        &self,
        user: Id,
        display_name: Option<&str>,
        bio: Option<&str>,
        avatar: Option<Option<&str>>,
    ) -> Result<User> {
        let row: UserRow = sqlx::query_as(&format!(
            "UPDATE users SET display_name = coalesce($2, display_name), bio = coalesce($3, bio), \
             avatar = CASE WHEN $4 THEN $5 ELSE avatar END WHERE id = $1 RETURNING {USER_COLS}"
        ))
        .bind(user)
        .bind(display_name)
        .bind(bio)
        .bind(avatar.is_some())
        .bind(avatar.flatten())
        .fetch_one(&self.pool)
        .await?;
        Ok(row.into())
    }

    pub async fn set_handle(&self, user: Id, handle: &str) -> Result<User> {
        let row: UserRow = sqlx::query_as(&format!("UPDATE users SET handle = $2 WHERE id = $1 RETURNING {USER_COLS}"))
            .bind(user)
            .bind(handle)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| taken(e, "users_handle", "That username"))?;
        Ok(row.into())
    }

    pub async fn set_status(&self, user: Id, status: &Status) -> Result<()> {
        sqlx::query("UPDATE users SET status_presence = $2, status_text = $3 WHERE id = $1")
            .bind(user)
            .bind(presence_text(status.presence))
            .bind(&status.text)
            .execute(&self.pool)
            .await?;
        Ok(())
    }

    pub async fn user(&self, id: Id) -> Result<User> {
        let row: Option<UserRow> = sqlx::query_as(&format!("SELECT {USER_COLS} FROM users WHERE id = $1"))
            .bind(id)
            .fetch_optional(&self.pool)
            .await?;
        row.map(User::from).ok_or(Error::NotFound)
    }

    pub async fn users(&self, ids: &[Id]) -> Result<Vec<User>> {
        let rows: Vec<UserRow> = sqlx::query_as(&format!("SELECT {USER_COLS} FROM users WHERE id = ANY($1)"))
            .bind(ids)
            .fetch_all(&self.pool)
            .await?;
        Ok(rows.into_iter().map(User::from).collect())
    }

    pub async fn user_by_handle(&self, handle: &str) -> Result<Option<User>> {
        let row: Option<UserRow> =
            sqlx::query_as(&format!("SELECT {USER_COLS} FROM users WHERE lower(handle) = lower($1)"))
                .bind(handle)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(User::from))
    }

    // ---- devices ----

    /// Register a device's public key, or return the device already holding
    /// it for this account (a device re-registering after a reinstall of the
    /// app that kept its key).
    pub async fn register_device(&self, user: Id, name: &str, public_key: &str) -> Result<Device> {
        let existing: Option<(Id, Id)> = sqlx::query_as("SELECT user_id, id FROM devices WHERE public_key = $1")
            .bind(public_key)
            .fetch_optional(&self.pool)
            .await?;
        match existing {
            Some((owner, id)) if owner == user => {
                let row: DeviceRow = sqlx::query_as(
                    "UPDATE devices SET name = $2, last_seen = now() WHERE id = $1 \
                     RETURNING id, name, public_key, created_at, last_seen",
                )
                .bind(id)
                .bind(name)
                .fetch_one(&self.pool)
                .await?;
                Ok(row.into())
            }
            Some(_) => Err(Error::Taken("That device key")),
            None => {
                let row: DeviceRow = sqlx::query_as(
                    "INSERT INTO devices (id, user_id, name, public_key) VALUES ($1, $2, $3, $4) \
                     RETURNING id, name, public_key, created_at, last_seen",
                )
                .bind(new_id())
                .bind(user)
                .bind(name)
                .bind(public_key)
                .fetch_one(&self.pool)
                .await
                .map_err(|e| taken(e, "devices_key", "That device key"))?;
                Ok(row.into())
            }
        }
    }

    pub async fn devices(&self, user: Id) -> Result<Vec<Device>> {
        let rows: Vec<DeviceRow> = sqlx::query_as(
            "SELECT id, name, public_key, created_at, last_seen FROM devices WHERE user_id = $1 ORDER BY created_at",
        )
        .bind(user)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(Device::from).collect())
    }

    /// A device and whose it is.
    pub async fn device(&self, id: Id) -> Result<(Id, Device)> {
        let row: Option<(Id, String, String, OffsetDateTime, OffsetDateTime, Id)> = sqlx::query_as(
            "SELECT id, name, public_key, created_at, last_seen, user_id FROM devices WHERE id = $1",
        )
        .bind(id)
        .fetch_optional(&self.pool)
        .await?;
        let (id, name, public_key, created_at, last_seen, user) = row.ok_or(Error::NotFound)?;
        Ok((user, DeviceRow { id, name, public_key, created_at, last_seen }.into()))
    }

    pub async fn remove_device(&self, user: Id, device: Id) -> Result<()> {
        let done = sqlx::query("DELETE FROM devices WHERE id = $1 AND user_id = $2")
            .bind(device)
            .bind(user)
            .execute(&self.pool)
            .await?;
        if done.rows_affected() == 0 {
            return Err(Error::NotFound);
        }
        Ok(())
    }

    pub async fn touch_device(&self, device: Id) -> Result<()> {
        sqlx::query("UPDATE devices SET last_seen = now() WHERE id = $1").bind(device).execute(&self.pool).await?;
        Ok(())
    }
}
