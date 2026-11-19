//! Account and first-run sessions. Bootstrap credentials cannot become an admin session.
use super::*;
use wm_proto::{AccountLoginResponse, AccountRole, AccountUser, SetupCompleteRequest,
    SetupCompleteResponse, SetupLoginResponse, SetupStatus};

pub struct SetupSession {
    server: String,
    cookie: String,
    csrf_token: String,
}
#[derive(Clone)]
pub struct AccountSession {
    server: String,
    cookie: String,
    csrf_token: String,
    user: AccountUser,
}
impl fmt::Debug for SetupSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SetupSession").field("credentials", &"[REDACTED]").finish()
    }
}
impl fmt::Debug for AccountSession {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("AccountSession").field("credentials", &"[REDACTED]").finish()
    }
}
impl AccountSession {
    pub fn user(&self) -> &AccountUser { &self.user }
    /// The server independently enforces the role on every admin request.
    pub fn as_admin(&self) -> Result<AdminSession> {
        if self.user.role != AccountRole::Admin { return Err(AdminError::InvalidSession); }
        let session = AdminSession { server: self.server.clone(), cookie: self.cookie.clone(),
            csrf_token: self.csrf_token.clone(), user: AdminUser {
                id: self.user.id, username: self.user.username.clone(),
            } };
        session.validate()?;
        Ok(session)
    }
}
fn cookie(response: &Response, name: &str) -> Result<String> {
    let prefix = format!("{name}=");
    let cookies: Vec<_> = response.headers().get_all(SET_COOKIE).iter()
        .filter_map(|value| value.to_str().ok()).filter_map(|value| value.split(';').next())
        .filter(|value| value.starts_with(&prefix)).collect();
    if cookies.len() != 1 { return Err(AdminError::InvalidSession); }
    let value = cookies[0];
    if value.len() <= prefix.len() || value.len() > 4096
        || value.bytes().any(|byte| byte <= 32 || byte >= 127) {
        return Err(AdminError::InvalidSession);
    }
    Ok(value.to_owned())
}
impl AdminClient {
    async fn account_request<T: DeserializeOwned>(&self, server: &str, cookie: &str, csrf: &str,
        method: Method, path: &str, body: Option<Value>) -> Result<T> {
        if server != self.server() { return Err(AdminError::ServerMismatch); }
        let mut cookie = HeaderValue::from_str(cookie).map_err(|_| AdminError::InvalidSession)?;
        let mut csrf = HeaderValue::from_str(csrf).map_err(|_| AdminError::InvalidSession)?;
        cookie.set_sensitive(true);
        csrf.set_sensitive(true);
        let mut request = self.http.request(method.clone(), self.server.join(path).expect("fixed path"))
            .header(COOKIE, cookie);
        if method != Method::GET { request = request.header("x-wm-csrf", csrf); }
        if let Some(body) = body { request = request.json(&body); }
        self.decode(request.send().await.map_err(AdminError::Transport)?).await
    }
    pub async fn setup_status(&self) -> Result<SetupStatus> {
        self.decode(self.http.get(self.server.join("api/v1/setup").expect("fixed path"))
            .send().await.map_err(AdminError::Transport)?).await
    }
    pub async fn setup_login(&self, username: &str, password: &str) -> Result<SetupSession> {
        let response = self.http.post(self.server.join("api/v1/setup/login").expect("fixed path"))
            .json(&json!({"username":username,"password":password}))
            .send().await.map_err(AdminError::Transport)?;
        let cookie = cookie(&response, "wm_setup");
        let body: SetupLoginResponse = self.decode(response).await?;
        if body.csrf_token.is_empty() { return Err(AdminError::InvalidSession); }
        Ok(SetupSession { server: self.server().into(), cookie: cookie?, csrf_token: body.csrf_token })
    }
    pub async fn complete_setup(&self, session: &SetupSession, request: &SetupCompleteRequest) -> Result<SetupCompleteResponse> {
        self.account_request(&session.server, &session.cookie, &session.csrf_token,
            Method::POST, "api/v1/setup/complete", Some(serde_json::to_value(request)?)).await
    }
    pub async fn account_login(&self, username: &str, password: &str) -> Result<AccountSession> {
        let response = self.http.post(self.server.join("api/v1/auth/login").expect("fixed path"))
            .json(&json!({"username":username,"password":password}))
            .send().await.map_err(AdminError::Transport)?;
        let cookie = cookie(&response, "wm_session");
        let body: AccountLoginResponse = self.decode(response).await?;
        if body.csrf_token.is_empty() { return Err(AdminError::InvalidSession); }
        Ok(AccountSession { server: self.server().into(), cookie: cookie?, csrf_token: body.csrf_token, user: body.user })
    }
    pub async fn account_session(&self, session: &AccountSession) -> Result<AccountLoginResponse> {
        self.account_request(&session.server, &session.cookie, &session.csrf_token,
            Method::GET, "api/v1/auth/session", None).await
    }
    pub async fn account_logout(&self, session: &AccountSession) -> Result<Value> {
        self.account_request(&session.server, &session.cookie, &session.csrf_token,
            Method::POST, "api/v1/auth/logout", None).await
    }
    pub async fn control_panel(&self, session: &AdminSession) -> Result<Value> {
        self.account_request(&session.server, &session.cookie, &session.csrf_token,
            Method::GET, "api/v1/extensions/control_panel/runtime", None).await
    }
    pub async fn set_control_panel(&self, session: &AdminSession, enabled: bool, digest: Option<&str>) -> Result<Value> {
        self.account_request(&session.server, &session.cookie, &session.csrf_token,
            Method::PUT, "api/v1/extensions/control_panel/runtime", Some(json!({"enabled":enabled,"digest":digest}))).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn account(role: AccountRole) -> AccountSession {
        AccountSession { server:"https://example.org/".into(), cookie:"wm_session=secret".into(),
            csrf_token:"secret-csrf".into(), user:AccountUser {id:Uuid::nil(),username:"person".into(),role} }
    }
    #[test]
    fn user_cannot_become_admin_and_secrets_are_redacted() {
        assert!(account(AccountRole::User).as_admin().is_err());
        assert!(account(AccountRole::Admin).as_admin().is_ok());
        assert!(!format!("{:?}",account(AccountRole::User)).contains("secret"));
        let setup=SetupSession{server:"https://example.org/".into(),cookie:"wm_setup=secret".into(),csrf_token:"secret".into()};
        assert!(!format!("{setup:?}").contains("secret"));
    }
    #[tokio::test]
    async fn account_and_setup_sessions_cannot_cross_servers() {
        let client=AdminClient::new("https://other.example.org").unwrap();
        assert!(matches!(client.account_session(&account(AccountRole::User)).await,Err(AdminError::ServerMismatch)));
        let setup=SetupSession{server:"https://example.org/".into(),cookie:"wm_setup=secret".into(),csrf_token:"secret".into()};
        let request=SetupCompleteRequest{admin:wm_proto::AccountCredentials{username:"admin".into(),password:"not-sent-secret".into()},user:wm_proto::AccountCredentials{username:"user".into(),password:"not-sent-secret".into()}};
        assert!(matches!(client.complete_setup(&setup,&request).await,Err(AdminError::ServerMismatch)));
    }
}
