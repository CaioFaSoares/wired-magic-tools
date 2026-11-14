//! Unprivileged UI side of the administrator-authorized macOS bridge.
use std::path::{Path, PathBuf};
use std::sync::mpsc::Sender;
use serde_json::Value;
use crate::{Msg, actions, app::Level};

pub fn socket() -> PathBuf {
    PathBuf::from(format!("/Library/Application Support/wired-magic-desktop-{}/agent.sock", unsafe { libc::geteuid() }))
}
pub fn is_socket(path: &Path) -> bool { cfg!(target_os = "macos") && path == socket() }

/// The prompt runs on this worker, never on the terminal event-loop thread.
/// Installation explicitly approves executing the current binary once as admin;
/// persistent root processes execute only its copied, root-owned helper.
pub fn connect(root: PathBuf, exe: PathBuf, start: bool, install: bool, tx: Sender<Msg>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static CONNECTING: AtomicBool = AtomicBool::new(false);
    if CONNECTING.swap(true, Ordering::SeqCst) {
        let _ = tx.send(Msg::Notify(Level::Info, "autorização/conexão desktop já em andamento".into()));
        return;
    }
    std::thread::spawn(move || {
        struct Reset;
        impl Drop for Reset { fn drop(&mut self) { CONNECTING.store(false, Ordering::SeqCst); } }
        let _reset = Reset;
        let result = (|| -> Result<(), String> {
            let endpoint = socket();
            if let Err(e) = std::fs::metadata(&root) {
                if e.kind() == std::io::ErrorKind::NotFound {
                    return Err("desktop incremental exige diretório root já existente com nó inscrito; enrollment/instalação fresh ainda não implementados".into());
                }
            }
            if let Err(error) = actions::local_call(&endpoint, "desktop_identity", Value::Null) {
                match crate::agent::probe_socket(&endpoint) {
                    Ok(crate::agent::SocketProbe::Listening) => return Err(error),
                    Err(e) => return Err(format!("IPC desktop não verificável: {e}; nenhuma instalação iniciada")),
                    Ok(crate::agent::SocketProbe::Absent) => {}
                }
                if !install {
                    return Err("bridge desktop ausente; pressione s para autorizar explicitamente instalar a cópia privilegiada desta versão e conectar (apenas nó root já inscrito)".into());
                }
                let root = root.to_str().ok_or("caminho do daemon precisa ser UTF-8")?;
                let args = vec!["desktop-helper".into(), "install".into(), "--root".into(), root.into(),
                    "--uid".into(), unsafe { libc::geteuid() }.to_string()];
                crate::authorization::authorize_command(&exe, &args)
                    .map_err(|e| format!("autorização desktop não concluída: {e}"))?;
            }
            let identity = actions::local_call(&endpoint, "desktop_identity", Value::Null)?;
            if identity.get("root").and_then(Value::as_str) != root.to_str()
                || identity.get("uid").and_then(Value::as_u64) != Some(unsafe { libc::geteuid() } as u64) {
                return Err("bridge já autorizado para outro root/UID; alteração exige administração explícita".into());
            }
            let metadata = actions::local_call(&endpoint, "desktop_metadata", Value::Null)?;
            let node = if metadata.is_null() { None } else {
                Some(serde_json::from_value(metadata).map_err(|e| format!("metadados inválidos: {e}"))?)
            };
            let _ = tx.send(Msg::Joined(node));
            if start { actions::local_call(&endpoint, "desktop_start", Value::Null)?; }
            Ok(())
        })();
        match result {
            Ok(()) => { let _ = tx.send(Msg::Notify(Level::Info, "bridge desktop autorizado; conectando ao agente (q mantém o daemon)".into())); }
            Err(e) => { let _ = tx.send(Msg::Notify(Level::Warn, e)); }
        }
    });
}
