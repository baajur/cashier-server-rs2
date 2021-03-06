use super::errors::{Error, Result};
use super::email::{register_user_email, update_user_email};
use actix_web::web::{self, block};
use chrono::{DateTime, Utc};
use derive_more::From;
use lettre::Transport;
use rand::{Rng, thread_rng};
use rand::distributions::{Alphanumeric, Distribution};
use serde::{Serialize, Deserialize};
use std::collections::{HashSet, HashMap};
use std::iter;
use tokio_postgres::{
    Client, Statement, types::Type,
    IsolationLevel, Row,
};
use crate::api::app_state::AppState;
use crate::api::extractors::auth::Auth;
use crate::queries::errors::Error::DuplicatedUser;

struct Digit;

impl Distribution<char> for Digit {
    fn sample<R: Rng + ?Sized>(&self, rng: &mut R) -> char {
        const RANGE: u32 = 10;
        const GEN_ASCII_STR_CHARSET: &[u8] = b"0123456789";
        GEN_ASCII_STR_CHARSET[(rng.next_u32() % RANGE) as usize] as char
    }
}

pub trait HasId {
    fn get_id(&self) -> i32;
}

#[derive(Debug)]
pub struct UserIdPasswordBlocked {
    pub id: i32,
    pub password: String,
    pub blocked: Option<bool>,
}

#[derive(Debug)]
pub struct UserIdCreatedAt {
    pub id: i32,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct UserCreatedByRegistration {
    pub id: i32,
    pub username: String,
    pub roles: Vec<String>,
    pub email: Option<String>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug)]
pub struct UserIdEmailUpdatedAt {
    pub id: i32,
    pub email: String,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Hash, Eq, PartialEq)]
pub struct PermissionSubjectAction {
    pub subject: String,
    pub action: String,
}

#[derive(Debug)]
pub struct UserAvatars {
    pub avatar: Option<String>,
    pub avatar128: Option<String>,
}

pub enum EitherUsernameOrEmail {
    Username(String),
    Email(String),
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserPublic {
    pub id: i32,
    pub username: String,
    pub nickname: Option<String>,
    pub avatar: Option<String>,
    pub avatar128: Option<String>,
    pub created_at: DateTime<Utc>,
}

impl From<&Row> for UserPublic {
    fn from(row: &Row) -> Self {
        Self {
            id: row.get("id"),
            username: row.get("username"),
            nickname: row.get("nickname"),
            avatar: row.get("avatar"),
            avatar128: row.get("avatar128"),
            created_at: row.get("created_at"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserWithoutRoles {
    pub id: i32,
    pub username: String,
    pub email: Option<String>,
    pub nickname: Option<String>,
    pub avatar: Option<String>,
    pub avatar128: Option<String>,
    pub blocked: Option<bool>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<&Row> for UserWithoutRoles {
    fn from(row: &Row) -> Self {
        Self {
            id: row.get("id"),
            username: row.get("username"),
            email: row.get("email"),
            nickname: row.get("nickname"),
            avatar: row.get("avatar"),
            avatar128: row.get("avatar128"),
            blocked: row.get("blocked"),
            created_at: row.get("created_at"),
            updated_at: row.get("updated_at"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct UserAll {
    pub id: i32,
    pub username: String,
    pub roles: Vec<i32>,
    pub email: Option<String>,
    pub nickname: Option<String>,
    pub avatar: Option<String>,
    pub avatar128: Option<String>,
    pub blocked: Option<bool>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl From<(UserWithoutRoles, Vec<i32>)> for UserAll {
    fn from(data: (UserWithoutRoles, Vec<i32>)) -> Self {
        Self {
            id: data.0.id,
            username: data.0.username,
            roles: data.1,
            email: data.0.email,
            nickname: data.0.nickname,
            avatar: data.0.avatar,
            avatar128: data.0.avatar128,
            blocked: data.0.blocked,
            created_at: data.0.created_at,
            updated_at: data.0.updated_at,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "populate-user")]
#[serde(rename_all = "kebab-case")]
pub enum UserAccessLevel {
    Public,
    WithoutRoles,
    All,
}

impl Default for UserAccessLevel {
    fn default() -> Self {
        UserAccessLevel::Public
    }
}

#[derive(From, Serialize, Deserialize, Debug)]
#[serde(tag = "access")]
#[serde(rename_all = "kebab-case")]
pub enum User {
    Public(UserPublic),
    WithoutRoles(UserWithoutRoles),
    All(UserAll),
}

impl User {
    pub fn map_avatars(&mut self, mapping: impl Fn(&String) -> String) {
        match self {
            User::Public(user) => {
                user.avatar = user.avatar.as_ref().map(&mapping);
                user.avatar128 = user.avatar128.as_ref().map(&mapping);
            }
            User::WithoutRoles(user) => {
                user.avatar = user.avatar.as_ref().map(&mapping);
                user.avatar128 = user.avatar128.as_ref().map(&mapping);
            }
            User::All(user) => {
                user.avatar = user.avatar.as_ref().map(&mapping);
                user.avatar128 = user.avatar128.as_ref().map(&mapping);
            }
        }
    }
}

impl HasId for User {
    fn get_id(&self) -> i32 {
        match self {
            User::Public(x) => x.id,
            User::WithoutRoles(x) => x.id,
            User::All(x) => x.id,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoleShort {
    id: i32,
    name: String,
}

impl From<&Row> for RoleShort {
    fn from(x: &Row) -> Self {
        Self {
            id: x.get("id"),
            name: x.get("name"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoleShortWithPermissions {
    id: i32,
    name: String,
    permissions: Vec<i32>,
}

impl From<(RoleShort, Vec<i32>)> for RoleShortWithPermissions {
    fn from(x: (RoleShort, Vec<i32>)) -> Self {
        Self {
            id: x.0.id,
            name: x.0.name,
            permissions: x.1,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoleWithoutPermissions {
    id: i32,
    name: String,
    display_name: String,
    description: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<&Row> for RoleWithoutPermissions {
    fn from(x: &Row) -> Self {
        Self {
            id: x.get("id"),
            name: x.get("name"),
            display_name: x.get("display_name"),
            description: x.get("description"),
            created_at: x.get("created_at"),
            updated_at: x.get("updated_at"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct RoleAll {
    id: i32,
    name: String,
    permissions: Vec<i32>,
    display_name: String,
    description: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<(RoleWithoutPermissions, Vec<i32>)> for RoleAll {
    fn from(x: (RoleWithoutPermissions, Vec<i32>)) -> Self {
        Self {
            id: x.0.id,
            name: x.0.name,
            permissions: x.1,
            display_name: x.0.display_name,
            description: x.0.description,
            created_at: x.0.created_at,
            updated_at: x.0.updated_at,
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "populate-role")]
#[serde(rename_all = "kebab-case")]
pub enum RoleAccessLevel {
    Short,
    ShortWithPermissions,
    WithoutPermissions,
    All,
}

#[derive(From, Serialize, Deserialize, Debug)]
#[serde(tag = "access")]
#[serde(rename_all = "kebab-case")]
pub enum Role {
    Short(RoleShort),
    ShortWithPermissions(RoleShortWithPermissions),
    WithoutPermissions(RoleWithoutPermissions),
    All(RoleAll),
}

impl HasId for Role {
    fn get_id(&self) -> i32 {
        match self {
            Role::Short(x) => x.id,
            Role::ShortWithPermissions(x) => x.id,
            Role::WithoutPermissions(x) => x.id,
            Role::All(x) => x.id,
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PermissionShort {
    id: i32,
    subject: String,
    action: String,
}

impl From<&Row> for PermissionShort {
    fn from(x: &Row) -> Self {
        Self {
            id: x.get("id"),
            subject: x.get("subject"),
            action: x.get("action"),
        }
    }
}

#[derive(Debug, Serialize, Deserialize)]
pub struct PermissionAll {
    id: i32,
    subject: String,
    action: String,
    display_name: String,
    description: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<&Row> for PermissionAll {
    fn from(x: &Row) -> Self {
        Self {
            id: x.get("id"),
            subject: x.get("subject"),
            action: x.get("action"),
            display_name: x.get("display_name"),
            description: x.get("description"),
            created_at: x.get("created_at"),
            updated_at: x.get("updated_at"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "populate-permission")]
#[serde(rename_all = "kebab-case")]
pub enum PermissionAccessLevel {
    Short,
    All,
}

#[derive(From, Serialize, Deserialize, Debug)]
#[serde(tag = "access")]
#[serde(rename_all = "kebab-case")]
pub enum Permission {
    Short(PermissionShort),
    All(PermissionAll),
}

#[derive(Debug, PartialEq, Clone)]
pub struct PermissionTree {
    map: HashMap<i32, HashMap<i32, PermissionSubjectAction>>,
}

impl PermissionTree {
    pub fn new(map: HashMap<i32, HashMap<i32, PermissionSubjectAction>>) -> Self {
        Self { map }
    }
    pub fn get(&self) -> HashMap<i32, PermissionSubjectAction> {
        self.map.values()
            .flat_map(|x| x.iter())
            .map(|x| (x.0.to_owned(), x.1.to_owned()))
            .collect()
    }
    pub fn get_subscribe(&self) -> HashSet<String> {
        self.get().values()
            .filter(|x| x.action == "subscribe")
            .map(|x| x.subject.clone())
            .collect()
    }
    pub fn add_role(&mut self, role: i32, permissions: HashMap<i32, PermissionSubjectAction>) {
        self.map.insert(role, permissions);
    }
    pub fn remove_role(&mut self, role: i32) {
        self.map.remove(&role);
    }
    pub fn add_permission(&mut self, role: i32, permission: i32, subject: String, action: String) {
        if let Some(permissions) = self.map.get_mut(&role) {
            permissions.insert(permission, PermissionSubjectAction {
                subject,
                action,
            });
        }
    }
    pub fn remove_permission(&mut self, role: i32, permission: i32) {
        if let Some(permissions) = self.map.get_mut(&role) {
            permissions.remove(&permission);
        }
    }
}

impl Default for PermissionTree {
    fn default() -> Self {
        Self { map: HashMap::new() }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserRegistration {
    pub id: String,
    pub code: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserRegistrationPublic {
    pub id: String,
    pub username: String,
    pub email: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub completed: Option<bool>,
}

impl From<&Row> for UserRegistrationPublic {
    fn from(row: &Row) -> Self {
        Self {
            id: row.get("id"),
            username: row.get("username"),
            email: row.get("email"),
            created_at: row.get("created_at"),
            expires_at: row.get("expires_at"),
            completed: row.get("completed"),
        }
    }
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserEmailUpdating {
    pub id: String,
    pub code: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
}

#[derive(Serialize, Deserialize, Debug)]
pub struct UserEmailUpdatingPublic {
    pub id: String,
    pub user: i32,
    pub new_email: String,
    pub created_at: DateTime<Utc>,
    pub expires_at: DateTime<Utc>,
    pub completed: Option<bool>,
}

impl From<&Row> for UserEmailUpdatingPublic {
    fn from(row: &Row) -> Self {
        Self {
            id: row.get("id"),
            user: row.get("user"),
            new_email: row.get("new_email"),
            created_at: row.get("created_at"),
            expires_at: row.get("expires_at"),
            completed: row.get("completed"),
        }
    }
}

pub struct Query {
    find_one_from_username_to_id_password_blocked: Statement,
    find_one_from_email_to_id_password_blocked: Statement,
    find_one_from_username_to_id: Statement,
    find_one_from_email_to_id: Statement,
    check_user_blocked: Statement,
    fetch_permission: Statement,
    fetch_default_permission: Statement,
    check_extra_roles: Statement,
    find_one_from_username_to_username_email: Statement,
    find_one_from_username_email_to_username_email: Statement,
    insert_one: Statement,
    insert_one_roles: Statement,
    fetch_avatars: Statement,
    update_avatars: Statement,
    find_one: Statement,
    find_one_public: Statement,
    find_one_to_username: Statement,
    find_roles_only_id: Statement,
    find_roles_short: Statement,
    find_roles_without_permissions: Statement,
    find_permissions_only_id: Statement,
    find_permissions_short: Statement,
    find_permissions_all: Statement,
    fetch_permission_tree: Statement,
    fetch_default_permission_tree: Statement,
    insert_one_into_user_registration: Statement,
    find_one_from_user_registration: Statement,
    find_one_from_user_registration_without_password: Statement,
    insert_one_registered_user: Statement,
    find_default_roles: Statement,
    complete_registration: Statement,
    query_registration: Statement,
    update_user: Statement,
    insert_one_into_user_email_updating: Statement,
    find_one_from_user_email_updating: Statement,
    find_one_from_user_email_updating_join_user: Statement,
    update_email: Statement,
    complete_email_updating: Statement,
    query_email_updating: Statement,
    find_one_to_password: Statement,
    update_password: Statement,
}

impl Query {
    pub async fn new(client: &Client) -> Self {
        let find_one_from_username_to_id_password_blocked = client.prepare_typed(
            "SELECT id, password, blocked FROM \"user\" \
                WHERE username = $1 AND NOT deleted LIMIT 1",
            &[Type::TEXT],
        ).await.unwrap();
        let find_one_from_email_to_id_password_blocked = client.prepare_typed(
            "SELECT id, password, blocked FROM \"user\" \
                WHERE email = $1 AND NOT deleted LIMIT 1",
            &[Type::TEXT],
        ).await.unwrap();
        let find_one_from_username_to_id = client.prepare_typed(
            "SELECT id FROM \"user\" \
                WHERE username = $1 AND NOT deleted LIMIT 1",
            &[Type::TEXT],
        ).await.unwrap();
        let find_one_from_email_to_id = client.prepare_typed(
            "SELECT id FROM \"user\" \
                WHERE email = $1 AND NOT deleted LIMIT 1",
            &[Type::TEXT],
        ).await.unwrap();
        let check_user_blocked = client.prepare_typed(
            "SELECT blocked FROM \"user\" \
                WHERE id = $1 AND NOT deleted LIMIT 1",
            &[Type::INT4],
        ).await.unwrap();
        let fetch_permission = client.prepare_typed(
            "SELECT DISTINCT subject, action from ( \
                    SELECT role.id from user_role, role \
                        WHERE user_role.user = $1 AND user_role.role = role.id AND NOT role.deleted \
                    UNION \
                    SELECT role.id from role \
                        WHERE role.name = 'default' AND NOT role.deleted \
                ) as role, role_permission, permission \
                    WHERE role.id = role_permission.role \
                    AND role_permission.permission = permission.id AND NOT permission.deleted",
            &[Type::INT4],
        ).await.unwrap();
        let fetch_default_permission = client.prepare(
            "SELECT DISTINCT subject, action from role, role_permission, permission \
                WHERE role.name = 'default' AND NOT role.deleted AND role.id = role_permission.role \
                AND role_permission.permission = permission.id AND NOT permission.deleted"
        ).await.unwrap();
        let check_extra_roles = client.prepare_typed(
            "SELECT UNNEST($1) EXCEPT \
                SELECT role.name from user_role, role WHERE user_role.user = $2 \
                AND user_role.role = role.id AND NOT role.deleted",
            &[Type::TEXT_ARRAY, Type::INT4]
        ).await.unwrap();
        let find_one_from_username_to_username_email = client.prepare_typed(
            "SELECT username, email FROM \"user\" \
                WHERE username = $1 AND NOT deleted LIMIT 1",
            &[Type::TEXT],
        ).await.unwrap();
        let find_one_from_username_email_to_username_email = client.prepare_typed(
            "SELECT username, email FROM \"user\" \
                WHERE (username = $1 OR email = $2) AND NOT deleted LIMIT 1",
            &[Type::TEXT, Type::TEXT],
        ).await.unwrap();
        let insert_one = client.prepare_typed(
            "INSERT INTO \"user\" (username, password, email, nickname, \
                                   created_at, updated_at, deleted) \
                VALUES ($1, $2, $3, $4, NOW(), NOW(), false) \
                RETURNING id, created_at",
            &[Type::TEXT, Type::TEXT, Type::TEXT, Type::TEXT],
        ).await.unwrap();
        let insert_one_roles = client.prepare_typed(
            "INSERT INTO user_role (\"user\", role) \
                SELECT $1, role.id FROM (SELECT UNNEST($2) AS role) AS temp, role \
                WHERE role.name = temp.role AND NOT role.deleted",
            &[Type::INT4, Type::TEXT_ARRAY],
        ).await.unwrap();
        let fetch_avatars = client.prepare_typed(
            "SELECT avatar, avatar128 FROM \"user\" \
                WHERE id = $1 AND NOT deleted LIMIT 1",
            &[Type::INT4],
        ).await.unwrap();
        let update_avatars = client.prepare_typed(
            "UPDATE \"user\" SET avatar = $1, avatar128 = $2, updated_at = NOW() \
                WHERE id = $3 AND NOT deleted \
                RETURNING updated_at",
            &[Type::TEXT, Type::TEXT, Type::INT4]
        ).await.unwrap();
        let find_one = client.prepare_typed(
            "SELECT id, username, email, nickname, avatar, avatar128, \
                        blocked, created_at, updated_at FROM \"user\" \
                WHERE id = $1 AND NOT deleted LIMIT 1",
            &[Type::INT4],
        ).await.unwrap();
        let find_one_public = client.prepare_typed(
            "SELECT id, username, nickname, avatar, avatar128, created_at FROM \"user\" \
                WHERE id = $1 AND NOT deleted LIMIT 1",
            &[Type::INT4],
        ).await.unwrap();
        let find_one_to_username = client.prepare_typed(
            "SELECT username FROM \"user\" \
                WHERE id = $1 AND NOT deleted LIMIT 1",
            &[Type::INT4],
        ).await.unwrap();
        let find_roles_only_id = client.prepare_typed(
            "SELECT DISTINCT \"user_id\", id \
                    FROM (SELECT UNNEST($1) AS user_id) AS temp, user_role, role \
                WHERE user_id = user_role.user AND role.id = user_role.role AND NOT role.deleted",
            &[Type::INT4_ARRAY],
        ).await.unwrap();
        let find_roles_short = client.prepare_typed(
            "SELECT DISTINCT \"user_id\", id, name \
                    FROM (SELECT UNNEST($1) AS user_id) AS temp, user_role, role \
                WHERE user_id = user_role.user AND role.id = user_role.role AND NOT role.deleted",
            &[Type::INT4_ARRAY],
        ).await.unwrap();
        let find_roles_without_permissions = client.prepare_typed(
            "SELECT DISTINCT \"user_id\", id, name, display_name, description, created_at, updated_at \
                    FROM (SELECT UNNEST($1) AS user_id) AS temp, user_role, role \
                WHERE user_id = user_role.user AND role.id = user_role.role AND NOT role.deleted",
            &[Type::INT4_ARRAY],
        ).await.unwrap();
        let find_permissions_only_id = client.prepare_typed(
            "SELECT DISTINCT role_id, id \
                    FROM (SELECT UNNEST($1) as role_id) as temp, role_permission, permission \
                WHERE role_id = role_permission.role AND permission.id = role_permission.permission \
                    AND NOT permission.deleted",
            &[Type::INT4_ARRAY],
        ).await.unwrap();
        let find_permissions_short = client.prepare_typed(
            "SELECT DISTINCT role_id, id, subject, action \
                    FROM (SELECT UNNEST($1) as role_id) as temp, role_permission, permission \
                WHERE role_id = role_permission.role AND permission.id = role_permission.permission \
                    AND NOT permission.deleted",
            &[Type::INT4_ARRAY],
        ).await.unwrap();
        let find_permissions_all = client.prepare_typed(
            "SELECT DISTINCT role_id, id, subject, action, display_name, description, \
                        created_at, updated_at \
                    FROM (SELECT UNNEST($1) as role_id) as temp, role_permission, permission \
                WHERE role_id = role_permission.role AND permission.id = role_permission.permission \
                    AND NOT permission.deleted",
            &[Type::INT4_ARRAY],
        ).await.unwrap();
        let fetch_permission_tree = client.prepare_typed(
            "SELECT DISTINCT role.id as role_id, permission.id as permission_id, subject, action from ( \
                    SELECT role.id from user_role, role \
                        WHERE user_role.user = $1 AND user_role.role = role.id AND NOT role.deleted \
                    UNION \
                    SELECT role.id from role \
                        WHERE role.name = 'default' AND NOT role.deleted \
                ) as role, role_permission, permission \
                    WHERE role.id = role_permission.role \
                    AND role_permission.permission = permission.id AND NOT permission.deleted",
            &[Type::INT4],
        ).await.unwrap();
        let fetch_default_permission_tree = client.prepare(
            "SELECT DISTINCT role.id as role_id, permission.id as permission_id, subject, action \
                    from role, role_permission, permission \
                WHERE role.name = 'default' AND NOT role.deleted AND role.id = role_permission.role \
                AND role_permission.permission = permission.id AND NOT permission.deleted"
        ).await.unwrap();
        let insert_one_into_user_registration = client.prepare_typed(
            &format!("INSERT INTO user_registration (id, code, username, password, \
                                                     email, created_at, expires_at) \
                VALUES ($1, $2, $3, $4, $5, NOW(), NOW() + INTERVAL '{}') \
                RETURNING created_at, expires_at", crate::constants::USER_REGISTRATION_EXPIRE),
            &[Type::TEXT, Type::TEXT, Type::TEXT, Type::TEXT, Type::TEXT],
        ).await.unwrap();
        let find_one_from_user_registration = client.prepare_typed(
            "SELECT code, username, password, email, expires_at FROM user_registration \
            WHERE id = $1 AND completed IS NULL LIMIT 1",
            &[Type::TEXT]
        ).await.unwrap();
        let find_one_from_user_registration_without_password = client.prepare_typed(
            "SELECT code, username, email, expires_at FROM user_registration \
            WHERE id = $1 AND completed IS NULL LIMIT 1",
            &[Type::TEXT]
        ).await.unwrap();
        let insert_one_registered_user = client.prepare_typed(
            "INSERT INTO \"user\" (username, password, email, \
                                   created_at, updated_at, deleted) \
                VALUES ($1, $2, $3, NOW(), NOW(), false) \
                RETURNING id, created_at",
            &[Type::TEXT, Type::TEXT, Type::TEXT],
        ).await.unwrap();
        let find_default_roles = client.prepare(
            "SELECT name FROM role \
            WHERE \"default\" = TRUE AND NOT deleted",
        ).await.unwrap();
        let complete_registration = client.prepare_typed(
            "UPDATE user_registration SET completed = TRUE \
             WHERE id = $1 AND completed IS NULL",
            &[Type::TEXT],
        ).await.unwrap();
        let query_registration = client.prepare_typed(
            "SELECT id, username, email, created_at, expires_at, completed \
            FROM user_registration WHERE id = $1 LIMIT 1",
            &[Type::TEXT]
        ).await.unwrap();
        let update_user = client.prepare_typed(
            "UPDATE \"user\" \
                SET username = CASE WHEN $1 THEN $2 ELSE username END, \
                    email = CASE WHEN $3 THEN $4 ELSE email END, \
                    nickname = CASE WHEN $5 THEN $6 ELSE nickname END, \
                    blocked = CASE WHEN $7 THEN $8 ELSE blocked END, \
                    updated_at = NOW() \
                WHERE id = $9 AND NOT DELETED \
                RETURNING updated_at",
            &[Type::BOOL, Type::TEXT, Type::BOOL, Type::TEXT,
                Type::BOOL, Type::TEXT, Type::BOOL, Type::BOOL, Type::INT4]
        ).await.unwrap();
        let insert_one_into_user_email_updating = client.prepare_typed(
            &format!("INSERT INTO user_email_updating (id, code, \"user\", new_email, \
                                                       created_at, expires_at) \
                VALUES ($1, $2, $3, $4, NOW(), NOW() + INTERVAL '{}') \
                RETURNING created_at, expires_at", crate::constants::USER_UPDATING_EMAIL_EXPIRE),
            &[Type::TEXT, Type::TEXT, Type::INT4, Type::TEXT]
        ).await.unwrap();
        let find_one_from_user_email_updating = client.prepare_typed(
            "SELECT code, \"user\", new_email, expires_at FROM user_email_updating \
            WHERE id = $1 AND completed IS NULL LIMIT 1",
            &[Type::TEXT]
        ).await.unwrap();
        let find_one_from_user_email_updating_join_user = client.prepare_typed(
            "SELECT code, new_email, \"user\".id as uid, \"user\".username, expires_at \
            FROM user_email_updating, \"user\" \
            WHERE user_email_updating.id = $1 AND completed IS NULL AND \
                user_email_updating.user = \"user\".id AND NOT \"user\".deleted \
            LIMIT 1",
            &[Type::TEXT],
        ).await.unwrap();
        let update_email = client.prepare_typed(
            "UPDATE \"user\" SET email = $1, updated_at = NOW() \
                WHERE id = $2 AND NOT deleted \
                RETURNING updated_at",
            &[Type::TEXT, Type::INT4]
        ).await.unwrap();
        let complete_email_updating = client.prepare_typed(
            "UPDATE user_email_updating SET completed = TRUE \
            WHERE id = $1 AND completed IS NULL",
            &[Type::TEXT]
        ).await.unwrap();
        let query_email_updating = client.prepare_typed(
            "SELECT id, \"user\", new_email, created_at, expires_at, completed \
            FROM user_email_updating WHERE id = $1 LIMIT 1",
            &[Type::TEXT]
        ).await.unwrap();
        let find_one_to_password = client.prepare_typed(
            "SELECT password FROM \"user\" \
                WHERE id = $1 AND NOT deleted LIMIT 1",
            &[Type::INT4]
        ).await.unwrap();
        let update_password = client.prepare_typed(
            "UPDATE \"user\" SET password = $1 \
                WHERE id = $2 AND NOT DELETED \
                RETURNING updated_at",
            &[Type::TEXT, Type::INT4]
        ).await.unwrap();
        Self {
            find_one_from_username_to_id_password_blocked,
            find_one_from_email_to_id_password_blocked,
            find_one_from_username_to_id,
            find_one_from_email_to_id,
            check_user_blocked,
            fetch_permission,
            fetch_default_permission,
            check_extra_roles,
            find_one_from_username_to_username_email,
            find_one_from_username_email_to_username_email,
            insert_one,
            insert_one_roles,
            fetch_avatars,
            update_avatars,
            find_one,
            find_one_public,
            find_one_to_username,
            find_roles_only_id,
            find_roles_short,
            find_roles_without_permissions,
            find_permissions_only_id,
            find_permissions_short,
            find_permissions_all,
            fetch_permission_tree,
            fetch_default_permission_tree,
            insert_one_into_user_registration,
            find_one_from_user_registration,
            find_one_from_user_registration_without_password,
            insert_one_registered_user,
            find_default_roles,
            complete_registration,
            query_registration,
            update_user,
            insert_one_into_user_email_updating,
            find_one_from_user_email_updating,
            find_one_from_user_email_updating_join_user,
            update_email,
            complete_email_updating,
            query_email_updating,
            find_one_to_password,
            update_password,
        }
    }
    pub async fn find_one_from_username_to_id_password_blocked(
        &self, client: &Client, username: &str,
    ) -> Result<UserIdPasswordBlocked> {
        let rows = client
            .query(&self.find_one_from_username_to_id_password_blocked, &[&username])
            .await?;
        let row = rows
            .get(0)
            .ok_or_else(|| Error::UserNotFound)?;
        Ok(UserIdPasswordBlocked {
            id: row.get("id"),
            password: row.get("password"),
            blocked: row.get("blocked"),
        })
    }
    pub async fn find_one_from_email_to_id_password_blocked(
        &self, client: &Client, email: &str,
    ) -> Result<UserIdPasswordBlocked> {
        let rows = client
            .query(&self.find_one_from_email_to_id_password_blocked, &[&email])
            .await?;
        let row = rows
            .get(0)
            .ok_or_else(|| Error::UserNotFound)?;
        Ok(UserIdPasswordBlocked {
            id: row.get("id"),
            password: row.get("password"),
            blocked: row.get("blocked"),
        })
    }
    pub async fn check_user_valid_by_id(
        &self, client: &Client, id: i32,
    ) -> Result<()>
    {
        let rows = client
            .query(&self.check_user_blocked, &[&id])
            .await?;
        let row = rows
            .get(0)
            .ok_or_else(|| Error::UserNotFound)?;
        if row.get::<&str, Option<bool>>("blocked").contains(&true) {
            return Err(Error::UserBlocked);
        }
        Ok(())
    }
    pub async fn fetch_permission(
        &self, client: &Client, id: i32,
    ) -> Result<Vec<PermissionSubjectAction>> {
        let rows = client
            .query(&self.fetch_permission, &[&id])
            .await?;
        Ok(rows.iter()
            .map(|row| PermissionSubjectAction {
                subject: row.get("subject"),
                action: row.get("action"),
            })
            .collect())
    }
    pub async fn fetch_default_permission(
        &self, client: &Client,
    ) -> Result<Vec<PermissionSubjectAction>> {
        let rows = client
            .query(&self.fetch_default_permission, &[])
            .await?;
        Ok(rows.iter()
            .map(|row| PermissionSubjectAction {
                subject: row.get("subject"),
                action: row.get("action"),
            })
            .collect())
    }
    pub async fn check_extra_roles(
        &self, client: &Client, id: i32, roles: &[String],
    ) -> Result<Vec<String>> {
        let rows = client
            .query(&self.check_extra_roles, &[&roles, &id])
            .await?;
        let results = rows.iter()
            .map(|x| x.get(0))
            .collect();
        Ok(results)
    }
    pub async fn insert_one(
        &self, client: &mut Client, username: &str, password: &str,
        roles: &[String], email: &Option<String>, nickname: &Option<String>,
    ) -> Result<UserIdCreatedAt> {
        let transaction = client.build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let duplicated_rows = match email {
            Some(email) => transaction
                .query(&self.find_one_from_username_email_to_username_email, &[&username, &email])
                .await?,
            None => transaction
                .query(&self.find_one_from_username_to_username_email, &[&username])
                .await?,
        };
        if let Some(row) = duplicated_rows.get(0) {
            return Err(Error::DuplicatedUser {
                field: if row.get::<&str, String>("username") == username { "username".into() }
                else { "email".into() }
            });
        }
        let password = String::from(password);
        let password = block(move || bcrypt::hash(password, crate::constants::BCRYPT_COST))
            .await?;
        let user = transaction
            .query_one(&self.insert_one, &[&username, &password, &email, &nickname])
            .await?;
        let id: i32 = user.get("id");
        if !roles.is_empty() {
            transaction
                .query(&self.insert_one_roles, &[&id, &roles])
                .await?;
        }
        transaction.commit()
            .await?;
        Ok(UserIdCreatedAt {
            id,
            created_at: user.get("created_at"),
        })
    }
    pub async fn check_user_valid(
        &self, client: &Client, credit: &EitherUsernameOrEmail, password: &str,
    ) -> Result<i32> {
        let user = match credit {
            EitherUsernameOrEmail::Username(username) =>
                self.find_one_from_username_to_id_password_blocked(client, username)
                    .await?,
            EitherUsernameOrEmail::Email(email) =>
                self.find_one_from_email_to_id_password_blocked(client, email)
                    .await?,
        };
        let password = String::from(password);
        let hash = user.password.clone();
        let verified = block(move || bcrypt::verify(password, &hash))
            .await?;
        if !verified {
            return Err(Error::WrongPassword);
        }
        if user.blocked.contains(&true) {
            return Err(Error::UserBlocked);
        }
        Ok(user.id)
    }
    pub async fn fetch_avatars(
        &self, client: &Client, id: i32,
    ) -> Result<UserAvatars> {
        let rows = client
            .query(&self.fetch_avatars, &[&id])
            .await?;
        let row = rows
            .get(0)
            .ok_or_else(|| Error::UserNotFound)?;
        Ok(UserAvatars {
            avatar: row.get("avatar"),
            avatar128: row.get("avatar128"),
        })
    }
    pub async fn update_avatars(
        &self, client: &Client, id: i32,
        avatar: &Option<String>, avatar128: &Option<String>
    ) -> Result<DateTime<Utc>> {
        let rows = client
            .query(&self.update_avatars, &[avatar, avatar128, &id])
            .await?;
        let row = rows
            .get(0)
            .ok_or_else(|| Error::UserNotFound)?;
        Ok(row.get("updated_at"))
    }
    pub async fn find_one_with_permissions_and_roles(
        &self, client: &mut Client, uid: i32,
        user_access_level: UserAccessLevel,
        role_access_level: Option<RoleAccessLevel>,
        permission_access_level: Option<PermissionAccessLevel>,
    ) -> Result<(User, Vec<Role>, Vec<Permission>)> {
        let transaction = client.build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let rows = match user_access_level {
            UserAccessLevel::Public => transaction
                .query(&self.find_one_public, &[&uid])
                .await?,
            UserAccessLevel::WithoutRoles | UserAccessLevel::All => transaction
                .query(&self.find_one, &[&uid])
                .await?,
        };
        let row = rows
            .get(0)
            .ok_or_else(|| Error::UserNotFound)?;
        let mut user: User = match user_access_level {
            UserAccessLevel::Public => UserPublic::from(row).into(),
            UserAccessLevel::WithoutRoles | UserAccessLevel::All =>
                UserWithoutRoles::from(row).into(),
        };
        let add_user_roles = match user_access_level {
            UserAccessLevel::All => true,
            _ => false,
        };
        let mut roles: Vec<Role> = Vec::new();
        let mut user2role: HashMap<i32, Vec<i32>> = HashMap::new();
        if add_user_roles || role_access_level.is_some() || permission_access_level.is_some() {
            match role_access_level {
                Some(RoleAccessLevel::Short) | Some(RoleAccessLevel::ShortWithPermissions) =>
                    for row in transaction.query(&self.find_roles_short, &[&vec![uid]]).await? {
                        let id = row.get("id");
                        roles.push(RoleShort::from(&row).into());
                        user2role.entry(row.get("user_id")).or_insert_with(Vec::new).push(id);
                    }
                Some(RoleAccessLevel::WithoutPermissions) | Some(RoleAccessLevel::All) =>
                    for row in transaction.query(&self.find_roles_without_permissions, &[&vec![uid]]).await? {
                        let id = row.get("id");
                        roles.push(RoleWithoutPermissions::from(&row).into());
                        user2role.entry(row.get("user_id")).or_insert_with(Vec::new).push(id);
                    }
                None => {
                    for row in transaction.query(&self.find_roles_only_id, &[&vec![uid]]).await? {
                        user2role.entry(row.get("user_id")).or_insert_with(Vec::new).push(row.get("id"));
                    }
                }
            }
        }
        if add_user_roles {
            user = match user {
                User::WithoutRoles(user) => UserAll::from((
                    user,
                    user2role.get(&uid).unwrap_or(&Vec::new()).clone()
                )).into(),
                _ => unreachable!(),
            }
        }
        let add_role_permissions = match role_access_level {
            Some(RoleAccessLevel::ShortWithPermissions) | Some(RoleAccessLevel::All) => true,
            _ => false,
        };
        let mut permissions: Vec<Permission> = Vec::new();
        let mut role2permission: HashMap<i32, Vec<i32>> = HashMap::new();
        if add_role_permissions || permission_access_level.is_some() {
            let mut roles_id: Vec<_> =  user2role.values()
                .flat_map(|x| x.iter().copied())
                .collect();
            roles_id.sort();
            roles_id.dedup();
            match permission_access_level {
                Some(PermissionAccessLevel::Short) => {
                    for row in transaction.query(&self.find_permissions_short, &[&roles_id]).await? {
                        let id = row.get("id");
                        permissions.push(PermissionShort::from(&row).into());
                        role2permission.entry(row.get("role_id")).or_insert_with(Vec::new).push(id);
                    }
                }
                Some(PermissionAccessLevel::All) => {
                    for row in transaction.query(&self.find_permissions_all, &[&roles_id]).await? {
                        let id = row.get("id");
                        permissions.push(PermissionAll::from(&row).into());
                        role2permission.entry(row.get("role_id")).or_insert_with(Vec::new).push(id);
                    }
                }
                None => {
                    for row in transaction.query(&self.find_permissions_only_id, &[&roles_id]).await? {
                        role2permission.entry(row.get("role_id")).or_insert_with(Vec::new).push(row.get("id"));
                    }
                }
            }
        }
        if add_role_permissions {
            roles = roles.into_iter()
                .map(|role| {
                    let id = role.get_id();
                    match role {
                        Role::Short(role) => RoleShortWithPermissions::from((
                            role,
                            role2permission.get(&id).unwrap_or(&Vec::new()).clone()
                        )).into(),
                        Role::WithoutPermissions(role) => RoleAll::from((
                            role,
                            role2permission.get(&id).unwrap_or(&Vec::new()).clone()
                        )).into(),
                        _ => unreachable!(),
                    }
                })
                .collect();
        }
        transaction.commit().await?;
        Ok((user, roles, permissions))
    }
    pub async fn fetch_permission_tree(
        &self, client: &Client, id: Option<i32>,
    ) -> Result<PermissionTree> {
        let rows = match id {
            Some(id) => client
                .query(&self.fetch_permission_tree, &[&id])
                .await?,
            None => client
                .query(&self.fetch_default_permission_tree, &[])
                .await?,
        };
        let mut tree = HashMap::new();
        for row in rows {
            let id: i32 = row.get("role_id");
            tree.entry(id)
                .or_insert_with(HashMap::new)
                .insert(row.get("permission_id"), PermissionSubjectAction {
                    subject: row.get("subject"),
                    action: row.get("action"),
                });
        }
        Ok(PermissionTree::new(tree))
    }
    pub async fn check_username_existence(
        &self, client: &Client, username: &str,
    ) -> Result<bool> {
        let rows = client
            .query(&self.find_one_from_username_to_id, &[&username])
            .await?;
        Ok(!rows.is_empty())
    }
    pub async fn check_email_existence(
        &self, client: &Client, email: &str,
    ) -> Result<bool> {
        let rows = client
            .query(&self.find_one_from_email_to_id, &[&email])
            .await?;
        Ok(!rows.is_empty())
    }
    pub async fn register_user(
        &self, client: &Client,
        app_data: web::Data<AppState>, // for smtp
        sender: &str, site: &str,
        username: &str, email: &str, password: &str,
    ) -> Result<UserRegistration> {
        let duplicated_rows = client
            .query(&self.find_one_from_username_email_to_username_email,
                   &[&username, &email])
            .await?;
        if let Some(row) = duplicated_rows.get(0) {
            return Err(Error::DuplicatedUser {
                field: if row.get::<&str, String>("username") == username { "username".into() }
                else { "email".into() }
            });
        }
        let mut rng = thread_rng();
        let id: String = iter::repeat(())
            .map(|()| rng.sample(Alphanumeric))
            .take(24)
            .collect();
        let code: String = iter::repeat(())
            .map(|()| rng.sample(Digit))
            .take(6)
            .collect();
        let message = register_user_email(sender.parse()?, email.parse()?,
                                        site, username, &id, &code)?;
        block(move || app_data.smtp.send(&message))
            .await?;
        let password = String::from(password);
        let password = block(move || bcrypt::hash(password, crate::constants::BCRYPT_COST))
            .await?;
        let row = client
            .query_one(&self.insert_one_into_user_registration,
                       &[&id, &code, &username, &password, &email])
            .await?;
        Ok(UserRegistration {
            id,
            code,
            created_at: row.get("created_at"),
            expires_at: row.get("expires_at"),
        })
    }
    pub async fn confirm_registration(
        &self, client: &mut Client, id: &str, code: &str,
    ) -> Result<UserCreatedByRegistration> {
        let transaction = client.build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let rows = transaction
            .query(&self.find_one_from_user_registration, &[&id])
            .await?;
        let row = rows
            .get(0)
            .ok_or_else(|| Error::UserRegistrationNotFound)?;
        let real_code: String = row.get("code");
        let username: String = row.get("username");
        let password: String = row.get("password");
        let email: String = row.get("email");
        let expires_at: DateTime<Utc> = row.get("expires_at");
        if expires_at < Utc::now() {
            return Err(Error::UserRegistrationExpired);
        }
        if real_code != code {
            return Err(Error::UserRegistrationWrongCode);
        }
        let duplicated_rows = transaction
            .query(&self.find_one_from_username_email_to_username_email,
                   &[&username, &email])
            .await?;
        if let Some(row) = duplicated_rows.get(0) {
            return Err(Error::DuplicatedUser {
                field: if row.get::<&str, String>("username") == username { "username".into() }
                else { "email".into() }
            });
        }
        let user = transaction
            .query_one(&self.insert_one_registered_user, &[&username, &password, &email])
            .await?;
        let user_id: i32 = user.get("id");
        let roles = transaction
            .query(&self.find_default_roles, &[])
            .await?
            .iter()
            .map(|x| x.get("name"))
            .collect::<Vec<String>>();
        transaction
            .query(&self.insert_one_roles, &[&user_id, &roles])
            .await?;
        transaction
            .query(&self.complete_registration, &[&id])
            .await?;
        transaction.commit().await?;
        Ok(UserCreatedByRegistration {
            id: user_id,
            username,
            roles,
            email: Some(email),
            created_at: user.get("created_at"),
        })
    }
    pub async fn query_registration(
        &self, client: &Client, id: &str,
    ) -> Result<UserRegistrationPublic> {
        let rows = client
            .query(&self.query_registration, &[&id])
            .await?;
        let row = rows
            .get(0)
            .ok_or_else(|| Error::UserRegistrationNotFound)?;
        let registration = UserRegistrationPublic::from(row);
        if registration.completed.is_none() && registration.expires_at < Utc::now() {
            return Err(Error::UserRegistrationExpired);
        }
        Ok(registration)
    }
    pub async fn resend_registration_email(
        &self, client: &Client,
        app_data: web::Data<AppState>,
        sender: &str, site: &str, id: &str,
    ) -> Result<()> {
        let rows = client
            .query(&self.find_one_from_user_registration_without_password, &[&id])
            .await?;
        let row = rows
            .get(0)
            .ok_or_else(|| Error::UserRegistrationNotFound)?;
        let code: String = row.get("code");
        let username: String = row.get("username");
        let email: String = row.get("email");
        let expires_at: DateTime<Utc> = row.get("expires_at");
        if expires_at < Utc::now() {
            return Err(Error::UserRegistrationExpired);
        }
        let message = register_user_email(sender.parse()?, email.parse()?,
                                          site, &username, &id, &code)?;
        block(move || app_data.smtp.send(&message))
            .await?;
        Ok(())
    }
    pub async fn update_user(
        &self, client: &mut Client, id: i32,
        username: &Option<String>, email: &Option<Option<String>>,
        nickname: &Option<Option<String>>, blocked: &Option<Option<bool>>,
    ) -> Result<DateTime<Utc>> {
        let transaction = client.build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .start()
            .await?;
        match username {
            Some(username) => if transaction
                .query(&self.find_one_from_username_to_id, &[&username])
                .await?
                .get(0)
                .map(|x| x.get::<&str, i32>("id"))
                .is_some() {
                return Err(DuplicatedUser { field: "username".into() });
            }
            _ => ()
        }
        match email {
            Some(Some(email)) => if transaction
                .query(&self.find_one_from_email_to_id, &[&email])
                .await?
                .get(0)
                .map(|x| x.get::<&str, i32>("id"))
                .is_some() {
                return Err(DuplicatedUser { field: "email".into() });
            }
            _ => ()
        }
        let enable_username = username.is_some();
        let enable_email = email.is_some();
        let enable_nickname = nickname.is_some();
        let enable_blocked = blocked.is_some();
        let rows = transaction
            .query(&self.update_user,
                   &[&enable_username, &username,
                       &enable_email, &email.clone().flatten(),
                       &enable_nickname, &nickname.clone().flatten(),
                       &enable_blocked, &blocked.clone().flatten(),
                   &id])
            .await?;
        let row = rows
            .get(0)
            .ok_or_else(|| Error::UserNotFound)?;
        transaction.commit()
            .await?;
        Ok(row.get("updated_at"))
    }
    pub async fn update_email(
        &self, client: &Client,
        app_data: web::Data<AppState>, // for smtp
        sender: &str, site: &str,
        uid: i32, new_email: &str,
    ) -> Result<UserEmailUpdating> {
        let username: String = client
            .query(&self.find_one_to_username, &[&uid])
            .await?
            .get(0)
            .ok_or_else(|| Error::UserNotFound)?
            .get("username");
        let rows = client
            .query(&self.find_one_from_email_to_id, &[&new_email])
            .await?;
        if !rows.is_empty() {
            return Err(Error::DuplicatedUser { field: "email".into() });
        }
        let mut rng = thread_rng();
        let id: String = iter::repeat(())
            .map(|()| rng.sample(Alphanumeric))
            .take(24)
            .collect();
        let code: String = iter::repeat(())
            .map(|()| rng.sample(Digit))
            .take(6)
            .collect();
        let message = update_user_email(sender.parse()?, new_email.parse()?,
                                        site, &username, &id, &code)?;
        block(move || app_data.smtp.send(&message))
            .await?;
        let row = client
            .query_one(&self.insert_one_into_user_email_updating,
                       &[&id, &code, &uid, &new_email])
            .await?;
        Ok(UserEmailUpdating {
            id,
            code,
            created_at: row.get("created_at"),
            expires_at: row.get("expires_at"),
        })
    }
    pub async fn confirm_email_updating(
        &self, client: &mut Client, auth: &Auth, id: &str, code: &str,
    ) -> Result<UserIdEmailUpdatedAt> {
        if !auth.has_permission("user-email-updating", "confirm-self") &&
            !auth.has_permission("user-email-updating", "confirm") {
            return Err(Error::PermissionDenied {
                subject: "user-email-updating".into(),
                action: "confirm".into(),
            })
        }
        let transaction = client.build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .start()
            .await?;
        let rows = transaction
            .query(&self.find_one_from_user_email_updating, &[&id])
            .await?;
        let row = rows
            .get(0)
            .ok_or_else(|| Error::UserEmailUpdatingNotFound)?;
        let real_code: String = row.get("code");
        let user: i32 = row.get("user");
        let new_email: String = row.get("new_email");
        let expires_at: DateTime<Utc> = row.get("expires_at");
        if (auth.claims.is_none() || auth.claims.as_ref().unwrap().uid != user) &&
            !auth.has_permission("user-email-updating", "confirm") {
            return Err(Error::PermissionDenied {
                subject: "user-email-updating".into(),
                action: "confirm".into(),
            })
        }
        if expires_at < Utc::now() {
            return Err(Error::UserEmailUpdatingExpired);
        }
        if real_code != code {
            return Err(Error::UserEmailUpdatingWrongCode);
        }
        let rows = transaction
            .query(&self.find_one_from_email_to_id, &[&new_email])
            .await?;
        if !rows.is_empty() {
            return Err(Error::DuplicatedUser { field: "email".into() });
        }
        let rows = transaction
            .query(&self.update_email, &[&new_email, &user])
            .await?;
        let row = rows
            .get(0)
            .ok_or_else(|| Error::UserNotFound)?;
        transaction
            .query(&self.complete_email_updating, &[&id])
            .await?;
        transaction.commit().await?;
        Ok(UserIdEmailUpdatedAt {
            id: user,
            email: new_email,
            updated_at: row.get("updated_at"),
        })
    }
    pub async fn query_email_updating(
        &self, client: &Client, auth: &Auth, id: &str,
    ) -> Result<UserEmailUpdatingPublic> {
        if !auth.has_permission("user-email-updating", "read-self") &&
            !auth.has_permission("user-email-updating", "read") {
            return Err(Error::PermissionDenied {
                subject: "user-email-updating".into(),
                action: "read".into(),
            })
        }
        let rows = client
            .query(&self.query_email_updating, &[&id])
            .await?;
        let row = rows
            .get(0)
            .ok_or_else(|| Error::UserEmailUpdatingNotFound)?;
        let registration = UserEmailUpdatingPublic::from(row);
        if (auth.claims.is_none() || auth.claims.as_ref().unwrap().uid != registration.user) &&
            !auth.has_permission("user-email-updating", "read") {
            return Err(Error::PermissionDenied {
                subject: "user-email-updating".into(),
                action: "read".into(),
            })
        }
        if registration.completed.is_none() && registration.expires_at < Utc::now() {
            return Err(Error::UserEmailUpdatingExpired);
        }
        Ok(registration)
    }
    pub async fn resend_email_updating_email(
        &self, client: &Client,
        app_data: web::Data<AppState>,
        auth: &Auth,
        sender: &str, site: &str, id: &str,
    ) -> Result<()> {
        if !auth.has_permission("user-email-updating", "resend-self") &&
            !auth.has_permission("user-email-updating", "resend") {
            return Err(Error::PermissionDenied {
                subject: "user-email-updating".into(),
                action: "resend".into(),
            })
        }
        let rows = client
            .query(&self.find_one_from_user_email_updating_join_user, &[&id])
            .await?;
        let row = rows
            .get(0)
            .ok_or_else(|| Error::UserEmailUpdatingNotFound)?;
        let code: String = row.get("code");
        let uid: i32 = row.get("uid");
        let username: String = row.get("username");
        let email: String = row.get("new_email");
        let expires_at: DateTime<Utc> = row.get("expires_at");
        if (auth.claims.is_none() || auth.claims.as_ref().unwrap().uid != uid) &&
            !auth.has_permission("user-email-updating", "resend") {
            return Err(Error::PermissionDenied {
                subject: "user-email-updating".into(),
                action: "resend".into(),
            })
        }
        if expires_at < Utc::now() {
            return Err(Error::UserEmailUpdatingExpired);
        }
        let message = update_user_email(sender.parse()?, email.parse()?,
                                        site, &username, &id, &code)?;
        block(move || app_data.smtp.send(&message))
            .await?;
        Ok(())
    }
    pub async fn update_password(
        &self, client: &mut Client, id: i32, password: String, old_password: Option<String>
    ) -> Result<DateTime<Utc>> {
        let transaction = client.build_transaction()
            .isolation_level(IsolationLevel::RepeatableRead)
            .start()
            .await?;
        if let Some(old_password) = old_password {
            let old_password_hash: String = transaction
                .query(&self.find_one_to_password, &[&id])
                .await?
                .get(0)
                .ok_or_else(|| Error::UserNotFound)?
                .get("password");
            let verified = block(move || bcrypt::verify(old_password, &old_password_hash))
                .await?;
            if !verified {
                return Err(Error::WrongPassword);
            }
        }
        let password_hash = block(move || bcrypt::hash(password, crate::constants::BCRYPT_COST))
            .await?;
        let updated_at = transaction
            .query(&self.update_password, &[&password_hash, &id])
            .await?
            .get(0)
            .ok_or_else(|| Error::UserNotFound)?
            .get("updated_at");
        transaction.commit().await?;
        Ok(updated_at)
    }
}
