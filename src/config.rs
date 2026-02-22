use std::sync::{Arc, Mutex};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct Entry {
    name:     String,
    ip:       String,
    port:     u16,
    username: String,
    password: String,
    dbtype:   String,
    #[serde(default)]
    database: String,
}

impl Entry {
    pub fn to_connection_url(&self) -> String {
        let db = if self.database.is_empty() {
            &self.username   
        } else {
            &self.database
        };

        match self.dbtype.as_str() {
            "mysql" | "mariadb" => format!(
                "mysql://{}:{}@{}:{}/{}",
                self.username, self.password, self.ip, self.port, db
            ),
            "postgres" | "postgresql" => format!(
                "postgres://{}:{}@{}:{}/{}",
                self.username, self.password, self.ip, self.port, db
            ),
            other => format!(
                "{}://{}:{}@{}:{}/{}",
                other, self.username, self.password, self.ip, self.port, db
            ),
        }
    }
}

pub struct Config {
    config_map: std::collections::HashMap<String, Entry>,
}

fn config_file_path() -> std::path::PathBuf {
    if let Some(mut home) = home_dir() {
        home.push(".db-mcp");
        home.push("config.json");
        home
    } else {
        std::path::PathBuf::from("db_config.json")
    }
}

fn home_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(std::path::PathBuf::from)
}

impl Config {
    pub fn new() -> Self {
        Self { config_map: std::collections::HashMap::new() }
    }

    pub fn add_entry(&mut self, name: String, ip: String, port: u16, username: String, password: String, dbtype: String, database: String,) {
        let entry = Entry {
            name: name.clone(),
            ip,
            port,
            username,
            password,
            dbtype,
            database,
        };
        self.config_map.insert(name, entry);
    }

    pub(crate) fn get_entry(&self, name: &str) -> Option<&Entry> {
        self.config_map.get(name)
    }

    pub fn get_connection_url(&self, name: &str) -> Option<String> {
        self.config_map.get(name).map(|e| e.to_connection_url())
    }

    pub fn load_from_file(&mut self) -> std::io::Result<()> {
        use std::fs::File;
        use std::io::{BufRead, BufReader};

        let path = config_file_path();
        let file = match File::open(&path) {
            Ok(f) => f,
            Err(ref e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(e) => return Err(e),
        };

        let reader = BufReader::new(file);
        for line in reader.lines() {
            let line = line?;
            let trimmed = line.trim();
            if trimmed.is_empty() {
                continue;
            }
            match serde_json::from_str::<Entry>(trimmed) {
                Ok(entry) => {
                    self.config_map.insert(entry.name.clone(), entry);
                }
                Err(e) => {
                    eprintln!("db-mcp: skipping malformed config line: {e}");
                }
            }
        }
        Ok(())
    }

    fn append_to_file(&self, entry: &Entry) -> std::io::Result<()> {
        use std::fs::{self, OpenOptions};
        use std::io::Write;

        let path = config_file_path();

        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }

        let json_line = serde_json::to_string(entry)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;

        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)?;

        file.write_all(json_line.as_bytes())?;
        file.write_all(b"\n")?;
        Ok(())
    }

    pub fn configure_server(&mut self, name: String, ip: String, port: u16, username: String, password: String, dbtype: String, database: String,) -> std::io::Result<String> {
        self.add_entry(
            name.clone(), ip, port, username, password, dbtype, database,
        );
        if let Some(entry) = self.get_entry(&name).cloned() {
            self.append_to_file(&entry)?;
            Ok(format!(
                "Server '{}' configured and saved to '{}'.",
                name,
                config_file_path().display()
            ))
        } else {
            Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "Failed to retrieve entry after insertion",
            ))
        }
    }
}

static CONFIG_INSTANCE: std::sync::OnceLock<Arc<Mutex<Config>>> =
    std::sync::OnceLock::new();

pub fn initialize_config() -> Result<(), String> {
    let mut cfg = Config::new();
    if let Err(e) = cfg.load_from_file() {
        eprintln!("db-mcp: warning — could not load config file: {e}");
    }
    let _ = CONFIG_INSTANCE.set(Arc::new(Mutex::new(cfg)));
    Ok(())
}

fn with_config<F, T>(f: F) -> Result<T, String>
where
    F: FnOnce(&mut Config) -> Result<T, String>,
{
    let arc = CONFIG_INSTANCE
        .get()
        .ok_or_else(|| "Config not initialized. Call initialize_config() first.".to_string())?;
    let mut cfg = arc.lock().map_err(|e| format!("Config lock poisoned: {e}"))?;
    f(&mut cfg)
}

pub fn add_permanent_entry(
    name:     String,
    ip:       String,
    port:     u16,
    username: String,
    password: String,
    dbtype:   String,
    database: String,
) -> Result<String, String> {
    with_config(|cfg| {
        cfg.configure_server(name, ip, port, username, password, dbtype, database)
            .map_err(|e| e.to_string())
    })
}

pub fn add_temporary_entry(name: String, ip: String, port: u16, username: String, password: String, dbtype: String, database: String,
) -> Result<String, String> {
    with_config(|cfg| {
        cfg.add_entry(name.clone(), ip, port, username, password, dbtype, database);
        Ok(format!("Connection '{}' registered (session only).", name))
    })
}

pub fn get_connection_url(name: &str) -> Option<String> {
    CONFIG_INSTANCE
        .get()?
        .lock()
        .ok()?
        .get_connection_url(name)
}