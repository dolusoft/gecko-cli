//! The CLI's files (Node: src/cli/config.ts; same names and JSON, so the Node and the
//! Rust build read each other's sign-in): the chosen environment (`config.json`), one
//! token per environment (`token-<env>.json`, the only secret on disk) and the bound
//! page's capability cache (`capabilities-<env>.json`).
//!
//! The token file is the user's alone: mode 0600 on POSIX (directory 0700), the user's
//! ACL only on Windows (`icacls`, as the Node build does).

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The signed-in user, as the hub names them.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct User {
    pub id: String,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct StoredToken {
    pub env: String,
    pub access_token: String,
    /// Milliseconds since the epoch.
    pub access_expires_at: u64,
    pub refresh_token: String,
    pub grant_id: String,
    pub user: User,
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

    pub fn read_default_env(&self) -> Option<String> {
        #[derive(Deserialize)]
        struct File {
            env: Option<String>,
        }
        read_json::<File>(&self.dir.join("config.json"))?.env
    }

    pub fn write_default_env(&self, env: &str) -> io::Result<()> {
        self.ensure_dir()?;
        let text = pretty(&serde_json::json!({ "env": env }))?;
        fs::write(self.dir.join("config.json"), text)
    }

    pub fn token_path(&self, env: &str) -> PathBuf {
        self.dir.join(format!("token-{}.json", file_key(env)))
    }

    fn capabilities_path(&self, env: &str) -> PathBuf {
        self.dir
            .join(format!("capabilities-{}.json", file_key(env)))
    }

    pub fn read_token(&self, env: &str) -> Option<StoredToken> {
        read_json(&self.token_path(env))
    }

    pub fn write_token(&self, token: &StoredToken) -> io::Result<()> {
        self.ensure_dir()?;
        write_secret(&self.token_path(&token.env), &pretty(token)?)
    }

    /// Removes the token and the capability cache of `env`; a missing file is fine.
    pub fn delete_token(&self, env: &str) -> io::Result<()> {
        for path in [self.token_path(env), self.capabilities_path(env)] {
            match fs::remove_file(&path) {
                Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                _ => {}
            }
        }
        Ok(())
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

/// An environment as a file name: anything but `A-Z a-z 0-9 . -` becomes `_`.
pub fn file_key(env: &str) -> String {
    env.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '.' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .collect()
}

/// Two-space JSON, as the Node build writes it.
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

    fn token(env: &str) -> StoredToken {
        StoredToken {
            env: env.to_owned(),
            access_token: "a".to_owned(),
            access_expires_at: 1,
            refresh_token: "r".to_owned(),
            grant_id: "g".to_owned(),
            user: User {
                id: "u1".to_owned(),
                name: "Zahit".to_owned(),
            },
        }
    }

    #[test]
    fn file_keys() {
        assert_eq!(file_key("127.0.0.1:4400"), "127.0.0.1_4400");
        assert_eq!(file_key("develop.example"), "develop.example");
        assert_eq!(file_key("a/b\\c ç"), "a_b_c__");
    }

    #[test]
    fn token_round_trip_in_the_node_format() {
        let config = temp_config("round-trip");
        assert_eq!(config.read_token("127.0.0.1:4400"), None);
        config.write_token(&token("127.0.0.1:4400")).unwrap();
        let path = config.token_path("127.0.0.1:4400");
        assert!(path.ends_with("token-127.0.0.1_4400.json"));
        let text = fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("{\n  \"env\": \"127.0.0.1:4400\",\n  \"accessToken\": \"a\",\n  \"accessExpiresAt\": 1,"), "{text}");
        assert_eq!(
            config.read_token("127.0.0.1:4400"),
            Some(token("127.0.0.1:4400"))
        );
        // No temporary file is left behind.
        let names: Vec<_> = fs::read_dir(config.dir())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names.len(), 1, "{names:?}");

        fs::write(config.dir().join("capabilities-127.0.0.1_4400.json"), "{}").unwrap();
        config.delete_token("127.0.0.1:4400").unwrap();
        assert!(fs::read_dir(config.dir()).unwrap().next().is_none());
        config.delete_token("127.0.0.1:4400").unwrap();
    }

    #[test]
    fn default_env() {
        let config = temp_config("default-env");
        assert_eq!(config.read_default_env(), None);
        config.write_default_env("develop.example").unwrap();
        assert_eq!(
            config.read_default_env().as_deref(),
            Some("develop.example")
        );
        fs::write(config.dir().join("config.json"), "not json").unwrap();
        assert_eq!(config.read_default_env(), None);
    }

    #[cfg(unix)]
    #[test]
    fn token_is_private() {
        use std::os::unix::fs::PermissionsExt;
        let config = temp_config("private");
        config.write_token(&token("e")).unwrap();
        let mode = |p: &Path| fs::metadata(p).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&config.token_path("e")), 0o600);
        assert_eq!(mode(config.dir()), 0o700);
    }
}
