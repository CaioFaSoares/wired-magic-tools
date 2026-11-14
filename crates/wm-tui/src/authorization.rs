//! Explicit, synchronous macOS administrator authorization, with no TUI wiring.
//!
//! Uses the system's native authorization prompt; neither Touch ID availability
//! nor a fresh prompt (authorization may be cached by macOS) is guaranteed.
//! Callers must trust the executable and arguments; an absolute executable path
//! is required. This is not a sandbox or a command allowlist.

use std::{io, path::Path};

/// Runs a command with administrator privileges using macOS authorization.
///
/// Blocks until completion. Standard input is closed; output is captured and
/// discarded on success. A failed command or declined authorization is an error.
/// Non-macOS platforms return [`io::ErrorKind::Unsupported`] without spawning.
/// macOS requires an absolute UTF-8 executable path and rejects embedded NUL bytes.
#[cfg(target_os = "macos")]
pub fn authorize_command(exe: &Path, args: &[String]) -> io::Result<()> {
    use std::process::{Command, Stdio};

    let script = authorization_script(exe, args)?;
    let output = Command::new("/usr/bin/osascript")
        .arg("-e")
        .arg(script)
        .stdin(Stdio::null())
        .output()
        .map_err(|error| {
            io::Error::new(
                error.kind(),
                format!("não foi possível executar /usr/bin/osascript: {error}"),
            )
        })?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    // osascript exposes AppleScript's user-canceled error number in stderr,
    // not as its process exit status. Keep all other failures as ordinary errors.
    let kind = if stderr.trim_end().ends_with("(-128)") {
        io::ErrorKind::Interrupted
    } else {
        io::ErrorKind::Other
    };
    Err(io::Error::new(
        kind,
        format!("falha no comando com privilégios de administrador ({}): {}", output.status, stderr.trim()),
    ))
}

#[cfg(not(target_os = "macos"))]
pub fn authorize_command(_exe: &Path, _args: &[String]) -> io::Result<()> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "a autorização nativa de administrador só está disponível no macOS",
    ))
}

/// Quotes one POSIX shell word, including the empty word, without expansion.
#[cfg(any(target_os = "macos", test))]
fn shell_quote(value: &str) -> io::Result<String> {
    if value.contains('\0') {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "caractere NUL no comando"));
    }
    Ok(format!("'{}'", value.replace('\'', "'\\''")))
}

/// Encodes data as an AppleScript string literal, never executable source.
#[cfg(any(target_os = "macos", test))]
fn applescript_quote(value: &str) -> String {
    let mut quoted = String::from("\"");
    for ch in value.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

#[cfg(any(target_os = "macos", test))]
fn authorization_script(exe: &Path, args: &[String]) -> io::Result<String> {
    if !exe.is_absolute() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "o caminho do executável deve ser absoluto",
        ));
    }
    let exe = exe.to_str().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "o caminho do executável deve ser UTF-8 válido")
    })?;
    let mut command = shell_quote(exe)?;
    for arg in args {
        command.push(' ');
        command.push_str(&shell_quote(arg)?);
    }
    Ok(format!(
        "do shell script {} with administrator privileges",
        applescript_quote(&command)
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shell_words_are_literal() {
        for (input, expected) in [
            ("", "''"),
            ("hello world", "'hello world'"),
            ("it's", "'it'\\''s'"),
            ("\"quoted\"", "'\"quoted\"'"),
            ("a\nb", "'a\nb'"),
            ("$HOME $(id); `id`", "'$HOME $(id); `id`'"),
            ("a\\b", "'a\\b'"),
        ] {
            assert_eq!(shell_quote(input).unwrap(), expected);
        }
    }

    #[test]
    fn applescript_escapes_quotes_backslashes_and_lines() {
        assert_eq!(
            applescript_quote("\"\n\r\t$\\'"),
            "\"\\\"\\n\\r\\t$\\\\'\""
        );
    }

    #[cfg(unix)]
    #[test]
    fn shell_quoting_roundtrips_through_real_shell() {
        use std::process::{Command, Stdio};

        for value in [
            "", "hello world", "it's", "\"quoted\"", "a\nb\r\tc",
            "$HOME $(printf injected); `printf injected`", "a\\b", "ação 🦀",
            "'\"\n$\\;|&<>()*?[]{}!#~",
        ] {
            // macOS uses /usr/bin/printf; some Unix systems provide /bin/printf.
            let printf = if Path::new("/bin/printf").is_file() {
                "/bin/printf"
            } else {
                "/usr/bin/printf"
            };
            let command = format!("{printf} %s {}", shell_quote(value).unwrap());
            let output = Command::new("/bin/sh")
                .arg("-c")
                .arg(command)
                .stdin(Stdio::null())
                .output()
                .unwrap();
            assert!(output.status.success(), "{value:?}: {:?}", output.stderr);
            assert_eq!(output.stdout, value.as_bytes(), "{value:?}");
            assert!(output.stderr.is_empty());
        }
    }

    #[test]
    fn rejects_relative_executable() {
        for path in ["echo", "./echo", "../bin/echo", ""] {
            assert_eq!(
                authorization_script(Path::new(path), &[]).unwrap_err().kind(),
                io::ErrorKind::InvalidInput
            );
        }
    }

    #[cfg(unix)]
    #[test]
    fn script_quotes_both_languages() {
        let script = authorization_script(
            Path::new("/tmp/a'b"),
            &["\"\n$\\".into(), String::new()],
        ).unwrap();
        assert_eq!(
            script,
            "do shell script \"'/tmp/a'\\\\''b' '\\\"\\n$\\\\' ''\" with administrator privileges"
        );
    }

    #[test]
    fn rejects_nul_and_empty_executable() {
        assert_eq!(shell_quote("a\0b").unwrap_err().kind(), io::ErrorKind::InvalidInput);
        assert!(authorization_script(Path::new(""), &[]).is_err());
        assert!(authorization_script(Path::new("/bin/echo"), &["\0".into()]).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn rejects_non_utf8_executable() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};
        assert!(authorization_script(Path::new(OsStr::from_bytes(b"/tmp/\xff")), &[]).is_err());
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unsupported_platform_never_spawns() {
        assert_eq!(
            authorize_command(Path::new("/not/an/executable"), &[]).unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
    }
}
