//! In-memory first-run form. Never renders password contents or persists them.
use super::*;

#[derive(Default)]
pub(super) struct SetupForm {
    fields: [String; 6],
    field: usize,
}
pub(super) enum FormAction {
    None,
    Invalid,
    Exit,
    Submit(SetupCompleteRequest),
}
impl SetupForm {
    fn valid(&self) -> bool {
        let f = &self.fields;
        let username = |s: &str| !s.trim().is_empty() && s.len() <= 64
            && s.trim().bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'));
        username(&f[0]) && username(&f[3]) && !f[0].trim().eq_ignore_ascii_case(f[3].trim())
            && f[1].len() >= 12 && f[4].len() >= 12
            && f[1] == f[2] && f[4] == f[5]
    }
    pub(super) fn key(&mut self, key: KeyEvent) -> FormAction {
        match key.code {
            KeyCode::Esc => return FormAction::Exit,
            KeyCode::Tab | KeyCode::Down => self.field = (self.field + 1) % 6,
            KeyCode::BackTab | KeyCode::Up => self.field = (self.field + 5) % 6,
            KeyCode::Backspace => { self.fields[self.field].pop(); }
            KeyCode::Enter => {
                if !self.valid() { return FormAction::Invalid; }
                let request = SetupCompleteRequest {
                    admin: AccountCredentials { username: self.fields[0].trim().into(), password: std::mem::take(&mut self.fields[1]) },
                    user: AccountCredentials { username: self.fields[3].trim().into(), password: std::mem::take(&mut self.fields[4]) },
                };
                self.fields[2].clear(); self.fields[5].clear();
                return FormAction::Submit(request);
            }
            KeyCode::Char(ch) if !key.modifiers.intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) && !ch.is_control() => {
                let limit = if matches!(self.field, 0 | 3) { 64 } else { 1024 };
                if self.fields[self.field].len() + ch.len_utf8() <= limit { self.fields[self.field].push(ch); }
            }
            _ => {}
        }
        FormAction::None
    }
    pub(super) fn body(&self) -> String {
        let names = ["Admin definitivo", "Senha admin", "Confirmar senha admin", "Usuário cotidiano", "Senha usuário", "Confirmar senha usuário"];
        let mut text = "A credencial temporária só pode concluir esta configuração.\nCrie duas contas distintas; senhas com 12+ caracteres.\nA conta cotidiana NÃO terá poderes de admin.\n\n".to_owned();
        for (i, name) in names.iter().enumerate() {
            let value = if matches!(i, 0 | 3) { safe_text(&self.fields[i]) } else { "*".repeat(self.fields[i].chars().count()) };
            text.push_str(&format!("{} {name}: {value}\n", if self.field == i { ">" } else { " " }));
        }
        text.push_str("\nApós confirmar: contas criadas atomicamente, bootstrap invalidado, login como usuário.");
        text
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    fn form() -> SetupForm {
        SetupForm { fields: ["operator".into(),"admin-secret-123".into(),"admin-secret-123".into(),
            "person".into(),"user-secret-123".into(),"user-secret-123".into()], field:0 }
    }
    #[test]
    fn masks_secrets_and_moves_them_out_on_submit() {
        let mut form = form();
        assert!(!form.body().contains("secret"));
        let action = form.key(KeyEvent::new(KeyCode::Enter,KeyModifiers::NONE));
        assert!(matches!(action,FormAction::Submit(request) if request.admin.username=="operator" && request.user.username=="person"));
        assert!(form.fields[1].is_empty() && form.fields[2].is_empty() && form.fields[4].is_empty() && form.fields[5].is_empty());
    }
    #[test]
    fn rejects_duplicate_short_and_mismatched_credentials() {
        let mut f = form(); f.fields[3]="operator".into(); assert!(!f.valid());
        let mut f = form(); f.fields[1]="short".into(); f.fields[2]="short".into(); assert!(!f.valid());
        let mut f = form(); f.fields[5]="different-secret".into(); assert!(!f.valid());
    }
}
