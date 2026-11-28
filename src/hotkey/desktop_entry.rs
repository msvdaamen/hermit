//! The `.desktop` entry the portal needs to identify Hermit.
//!
//! Portals resolve an app id to its desktop entry; without one, registering the
//! app id fails and KDE refuses to bind shortcuts for an anonymous app.

use std::path::PathBuf;
use std::{env, fs};

use anyhow::Context as _;

use crate::telemetry::private_path;

/// Writes `$XDG_DATA_HOME/applications/<APP_ID>.desktop`, pointing at the running
/// executable. Leaves the file alone when it is already up to date.
pub(super) fn ensure_installed() -> anyhow::Result<()> {
    let executable = env::current_exe().context("locating the hermit executable")?;
    let contents = format!(
        "[Desktop Entry]\n\
         Type=Application\n\
         Name=Hermit\n\
         Comment=Keyboard-driven launcher\n\
         Exec={}\n\
         NoDisplay=true\n",
        quote_exec_argument(&executable.to_string_lossy()),
    );

    let path = applications_dir()?.join(format!("{}.desktop", crate::APP_ID));
    if fs::read_to_string(&path).is_ok_and(|existing| existing == contents) {
        return Ok(());
    }
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).with_context(|| format!("creating {}", private_path(parent)))?;
    }
    fs::write(&path, contents).with_context(|| format!("writing {}", private_path(&path)))?;
    tracing::info!("installed desktop entry at {}", private_path(&path));
    Ok(())
}

fn applications_dir() -> anyhow::Result<PathBuf> {
    let data_home = match env::var_os("XDG_DATA_HOME").filter(|dir| !dir.is_empty()) {
        Some(dir) => PathBuf::from(dir),
        None => PathBuf::from(env::var_os("HOME").context("HOME is not set")?).join(".local/share"),
    };
    Ok(data_home.join("applications"))
}

/// Quotes one `Exec` argument per the Desktop Entry specification.
fn quote_exec_argument(argument: &str) -> String {
    let mut quoted = String::with_capacity(argument.len() + 2);
    quoted.push('"');
    for character in argument.chars() {
        if matches!(character, '"' | '`' | '$' | '\\') {
            quoted.push('\\');
        }
        // `%` introduces a field code even inside quotes.
        if character == '%' {
            quoted.push('%');
        }
        quoted.push(character);
    }
    quoted.push('"');
    quoted
}

#[cfg(test)]
mod tests {
    use super::quote_exec_argument;

    #[test]
    fn quotes_plain_path() {
        assert_eq!(
            quote_exec_argument("/usr/bin/hermit"),
            "\"/usr/bin/hermit\""
        );
    }

    #[test]
    fn escapes_reserved_characters() {
        assert_eq!(
            quote_exec_argument("/home/a b/$x\"%"),
            "\"/home/a b/\\$x\\\"%%\""
        );
    }
}
