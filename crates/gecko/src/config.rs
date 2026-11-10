//! The CLI's files: the hubs the user trusted (`config.json`) and the one binding's token
//! (`token.json`, the only secret on disk). A binding and its token live and end
//! together (attach spec §6), so there is one token at most. Files of the 0.3.0 layout
//! (`token-<env>.json`, `config.json` with `env`) are not read.
//!
//! The token file is the user's alone: mode 0600 on POSIX (directory 0700), the user's
//! ACL only on Windows (`icacls`).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The user the binding acts for, as the hub names them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
}

/// The token of the binding, with the hub that issued it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredToken {
    /// The hub's origin, as the attach code named it (`http://127.0.0.1:4400`).
    pub hub: String,
    pub access_token: String,
    /// Milliseconds since the epoch.
    pub access_expires_at: u64,
    pub refresh_token: String,
    pub grant_id: String,
    pub binding_id: String,
    /// The approval level chosen in the tab (`ask-server`, `never-ask`).
    pub level: String,
    pub user: User,
}

/// `config.json`: the settings that are not secret.
#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Settings {
    /// Hub origins the user agreed to connect to (attach spec §3.3).
    #[serde(default)]
    trusted_hubs: Vec<String>,
}

/// The directory of the CLI's files.
#[derive(Debug, Clone)]
pub struct Config {
    dir: PathBuf,
}

impl Config {
    /// `GECKO_CONFIG_DIR`, else `%APPDATA%\gecko-cli` on Windows and
    /// `$XDG_CONFIG_HOME/gecko-cli` or `~/.config/gecko-cli` elsewhere.
    pub fn from_env() -> Config {
        let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
        if let Some(dir) = var("GECKO_CONFIG_DIR") {
            return Config::at(dir);
        }
        let home = || {
            var(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(PathBuf::from)
                .unwrap_or_default()
        };
        let base = if cfg!(windows) {
            var("APPDATA")
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join("AppData").join("Roaming"))
        } else {
            var("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| home().join(".config"))
        };
        Config::at(base.join("gecko-cli"))
    }

    pub fn at(dir: impl Into<PathBuf>) -> Config {
        Config { dir: dir.into() }
    }

    #[cfg(test)]
    pub fn dir(&self) -> &Path {
        &self.dir
    }

    fn settings_path(&self) -> PathBuf {
        self.dir.join("config.json")
    }

    /// Settings that cannot be read (missing, an older layout, broken) read as none.
    fn settings(&self) -> Settings {
        read_json(&self.settings_path()).unwrap_or_default()
    }

    /// Whether the user trusted this hub origin before.
    pub fn trusts(&self, hub: &str) -> bool {
        self.settings().trusted_hubs.iter().any(|h| h == hub)
    }

    /// Adds a hub origin to the trusted list.
    pub fn trust(&self, hub: &str) -> io::Result<()> {
        let mut settings = self.settings();
        if settings.trusted_hubs.iter().any(|h| h == hub) {
            return Ok(());
        }
        settings.trusted_hubs.push(hub.to_owned());
        self.ensure_dir()?;
        fs::write(self.settings_path(), pretty(&settings)?)
    }

    pub fn token_path(&self) -> PathBuf {
        self.dir.join("token.json")
    }

    /// The binding's token; a file that cannot be read counts as none.
    pub fn read_token(&self) -> Option<StoredToken> {
        read_json(&self.token_path())
    }

    pub fn write_token(&self, token: &StoredToken) -> io::Result<()> {
        self.ensure_dir()?;
        write_secret(&self.token_path(), &pretty(token)?)
    }

    /// Removes the token; a missing file is fine.
    pub fn delete_token(&self) -> io::Result<()> {
        match fs::remove_file(self.token_path()) {
            Err(error) if error.kind() != io::ErrorKind::NotFound => Err(error),
            _ => Ok(()),
        }
    }

    fn ensure_dir(&self) -> io::Result<()> {
        if self.dir.is_dir() {
            return Ok(());
        }
        create_private_dir(&self.dir)?;
        restrict_windows(&self.dir, true);
        Ok(())
    }
}

/// Two-space JSON.
fn pretty<T: Serialize>(value: &T) -> io::Result<String> {
    serde_json::to_string_pretty(value).map_err(io::Error::other)
}

fn read_json<T: for<'de> Deserialize<'de>>(path: &Path) -> Option<T> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

#[cfg(unix)]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    use std::os::unix::fs::DirBuilderExt;
    fs::DirBuilder::new()
        .recursive(true)
        .mode(0o700)
        .create(dir)
}

#[cfg(not(unix))]
fn create_private_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)
}

/// Writes through a temporary file and a rename, so a reader never sees half a token.
fn write_secret(path: &Path, text: &str) -> io::Result<()> {
    let temp = path.with_extension(format!("json.{}.tmp", std::process::id()));
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temp)?;
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        drop(file);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&temp, fs::Permissions::from_mode(0o600))?;
        }
        restrict_windows(&temp, false);
        fs::rename(&temp, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temp);
    }
    result
}

/// Removes inherited rights and grants the current user alone. The file stays in the
/// user's profile either way; the ACL is a second fence, so a failure is ignored.
#[cfg(windows)]
fn restrict_windows(path: &Path, directory: bool) {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    let Some(user) = std::env::var_os("USERNAME").filter(|u| !u.is_empty()) else {
        return;
    };
    let mut grant = user;
    grant.push(if directory { ":(OI)(CI)F" } else { ":F" });
    let _ = Command::new("icacls")
        .arg(path)
        .arg("/inheritance:r")
        .arg("/grant:r")
        .arg(grant)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .creation_flags(CREATE_NO_WINDOW)
        .status();
}

#[cfg(not(windows))]
fn restrict_windows(_path: &Path, _directory: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_config(name: &str) -> Config {
        let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/test-tmp/config")
            .join(format!("{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        Config::at(dir)
    }

    fn token() -> StoredToken {
        StoredToken {
            hub: "http://127.0.0.1:4400".to_owned(),
            access_token: "a".to_owned(),
            access_expires_at: 1,
            refresh_token: "r".to_owned(),
            grant_id: "g".to_owned(),
            binding_id: "b".to_owned(),
            level: "ask-server".to_owned(),
            user: User {
                id: "u1".to_owned(),
                name: "Zahit".to_owned(),
            },
        }
    }

    #[test]
    fn token_round_trip() {
        let config = temp_config("round-trip");
        assert_eq!(config.read_token(), None);
        config.write_token(&token()).unwrap();
        let path = config.token_path();
        assert!(path.ends_with("token.json"));
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.starts_with("{\n  \"hub\": \"http://127.0.0.1:4400\",\n  \"accessToken\": \"a\","),
            "{text}"
        );
        assert_eq!(config.read_token(), Some(token()));
        // No temporary file is left behind.
        let names: Vec<_> = fs::read_dir(config.dir())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");

        config.delete_token().unwrap();
        assert!(fs::read_dir(config.dir()).unwrap().next().is_none());
        config.delete_token().unwrap();
    }

    #[test]
    fn an_older_layout_is_not_read() {
        let config = temp_config("older");
        fs::create_dir_all(config.dir()).unwrap();
        // A 0.3.0 token has no hub and no binding.
        fs::write(
            config.token_path(),
            r#"{"env":"127.0.0.1:4400","accessToken":"a","accessExpiresAt":1,"refreshToken":"r","grantId":"g","user":{"id":"u","name":"n"}}"#,
        )
        .unwrap();
        assert_eq!(config.read_token(), None);
        fs::write(config.dir().join("config.json"), r#"{"env":"x"}"#).unwrap();
        assert!(!config.trusts("http://127.0.0.1:4400"));
        config.trust("http://127.0.0.1:4400").unwrap();
        assert!(config.trusts("http://127.0.0.1:4400"));
    }

    #[test]
    fn trusted_hubs_are_a_list() {
        let config = temp_config("trusted");
        assert!(!config.trusts("https://a.example"));
        config.trust("https://a.example").unwrap();
        config.trust("https://b.example").unwrap();
        config.trust("https://a.example").unwrap();
        assert!(config.trusts("https://a.example"));
        assert!(config.trusts("https://b.example"));
        let text = fs::read_to_string(config.dir().join("config.json")).unwrap();
        assert_eq!(text.matches("a.example").count(), 1, "{text}");
        fs::write(config.dir().join("config.json"), "not json").unwrap();
        assert!(!config.trusts("https://a.example"));
    }

    #[cfg(unix)]
    #[test]
    fn token_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let config = temp_config("private");
        config.write_token(&token()).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&config.token_path()), 0o600);
        assert_eq!(mode(config.dir()), 0o700);
    }
}
