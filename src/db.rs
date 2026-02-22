use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use sqlx::{any::AnyPoolOptions, AnyPool, Column, Row, TypeInfo};
use std::sync::Arc;
use tokio::sync::Mutex;
use tracing::info;

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum DbKind {
    MySQL,
    Postgres,
}

impl DbKind {
    pub fn from_url(url: &str) -> Result<Self> {
        if url.starts_with("mysql://") || url.starts_with("mariadb://") {
            Ok(Self::MySQL)
        } else if url.starts_with("postgres://") || url.starts_with("postgresql://") {
            Ok(Self::Postgres)
        } else {
            Err(anyhow!(
                "Unsupported scheme. Use mysql:// or postgres:// connection strings."
            ))
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            DbKind::MySQL => "MySQL",
            DbKind::Postgres => "PostgreSQL",
        }
    }
}

pub struct ConfigVsDBstate {
	user_vs_db : std::collections::HashMap<String, SharedState>
}

impl ConfigVsDBstate {
	pub fn new() -> Self {
		Self { user_vs_db: std::collections::HashMap::new() }
	}

	pub fn get(&self, name: &str) -> Option<SharedState> {
		self.user_vs_db.get(name).cloned()
	}

	pub fn get_first(&self) -> Option<SharedState> {
		self.user_vs_db.values().next().cloned()
	}

	#[allow(dead_code)]
	pub fn has_config(&self, user: &str) -> bool {
		self.user_vs_db.contains_key(user)
	}

	#[allow(dead_code)]
	pub fn has_any(&self) -> bool {
		!self.user_vs_db.is_empty()
	}

	pub fn add(&mut self, user: String, db_state: SharedState) {
		self.user_vs_db.insert(user, db_state);
	}

	pub fn remove(&mut self, name: &str) -> Option<SharedState> {
		self.user_vs_db.remove(name)
	}

	pub fn names(&self) -> Vec<String> {
		self.user_vs_db.keys().cloned().collect()
	}
}

pub type ConfigSharedState = Arc<Mutex<ConfigVsDBstate>>;

pub struct DbState {
    pub pool: Option<AnyPool>,
    pub kind: Option<DbKind>,
    pub url: Option<String>,
}

impl DbState {
    pub fn new() -> Self {
        Self { pool: None, kind: None, url: None }
    }

    pub fn connected(&self) -> bool {
        self.pool.is_some()
    }

    pub fn pool(&self) -> Result<&AnyPool> {
        self.pool.as_ref().ok_or_else(|| {
            anyhow!("Not connected. Call connect_database first.")
        })
    }

    pub fn kind(&self) -> Result<DbKind> {
        self.kind.ok_or_else(|| anyhow!("Not connected."))
    }
}

pub type SharedState = Arc<Mutex<DbState>>;

pub async fn connect(state: &SharedState, url: &str) -> Result<String> {
    let kind = DbKind::from_url(url)?;

    let pool = AnyPoolOptions::new()
        .max_connections(5)
        .connect(url)
        .await
        .map_err(|e| anyhow!("Connection failed: {e}"))?;

    let mut st = state.lock().await;
    if let Some(old) = st.pool.take() {
        old.close().await;
    }
    st.pool = Some(pool);
    st.kind = Some(kind);
    st.url = Some(url.to_string());

    info!("Connected to {} at {url}", kind.label());
    Ok(format!("Connected to {} ({})", kind.label(), redact_url(url)))
}

pub async fn disconnect(state: &SharedState) -> Result<String> {
    let mut st = state.lock().await;
    if let Some(pool) = st.pool.take() {
        pool.close().await;
        st.kind = None;
        st.url = None;
        Ok("Disconnected from database.".into())
    } else {
        Ok("No active connection.".into())
    }
}

pub async fn execute_query(state: &SharedState, sql: &str) -> Result<Value> {
    let st = state.lock().await;
    let pool = st.pool()?;

    let trimmed = sql.trim().to_uppercase();
    let is_select = trimmed.starts_with("SELECT")
        || trimmed.starts_with("SHOW")
        || trimmed.starts_with("DESCRIBE")
        || trimmed.starts_with("EXPLAIN")
        || trimmed.starts_with("WITH");

    if is_select {
        let rows = sqlx::query(sql)
            .fetch_all(pool)
            .await
            .map_err(|e| anyhow!("Query error: {e}"))?;

        let result: Vec<Value> = rows.iter().map(row_to_json).collect();
        Ok(json!({
            "rows": result,
            "row_count": result.len()
        }))
    } else {
        let res = sqlx::query(sql)
            .execute(pool)
            .await
            .map_err(|e| anyhow!("Query error: {e}"))?;

        Ok(json!({
            "rows_affected": res.rows_affected(),
            "message": format!("Query executed successfully. {} row(s) affected.", res.rows_affected())
        }))
    }
}


pub async fn list_databases(state: &SharedState) -> Result<Value> {
    let st = state.lock().await;
    let pool = st.pool()?;
    let kind = st.kind()?;

    let sql = match kind {
        DbKind::MySQL =>
            "SELECT schema_name AS `database` FROM information_schema.schemata ORDER BY schema_name",
        DbKind::Postgres =>
            "SELECT datname AS database FROM pg_database WHERE datistemplate = false ORDER BY datname",
    };

    let rows = sqlx::query(sql).fetch_all(pool).await?;
    let dbs: Vec<String> = rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>(0).ok())
        .collect();

    Ok(json!({ "databases": dbs }))
}

pub async fn list_tables(state: &SharedState) -> Result<Value> {
    let st = state.lock().await;
    let pool = st.pool()?;
    let kind = st.kind()?;

    let sql = match kind {
        DbKind::MySQL => {
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema = DATABASE() AND table_type = 'BASE TABLE' \
             ORDER BY table_name"
        }
        DbKind::Postgres => {
            "SELECT table_name FROM information_schema.tables \
             WHERE table_schema NOT IN ('pg_catalog','information_schema') \
             AND table_type = 'BASE TABLE' \
             ORDER BY table_name"
        }
    };

    let rows = sqlx::query(sql).fetch_all(pool).await?;
    let tables: Vec<String> = rows
        .iter()
        .filter_map(|r| r.try_get::<String, _>(0).ok())
        .collect();

    Ok(json!({ "tables": tables }))
}

pub async fn describe_table(state: &SharedState, table: &str) -> Result<Value> {
    let st = state.lock().await;
    let pool = st.pool()?;
    let kind = st.kind()?;

    let sql = match kind {
        DbKind::MySQL => format!(
            "SELECT column_name, data_type, is_nullable, column_default, \
             character_maximum_length, column_key, extra \
             FROM information_schema.columns \
             WHERE table_schema = DATABASE() AND table_name = '{table}' \
             ORDER BY ordinal_position"
        ),
        DbKind::Postgres => format!(
            "SELECT column_name, data_type, is_nullable, column_default, \
             character_maximum_length \
             FROM information_schema.columns \
             WHERE table_name = '{table}' \
             ORDER BY ordinal_position"
        ),
    };

    let rows = sqlx::query(&sql).fetch_all(pool).await
        .map_err(|e| anyhow!("describe_table error: {e}"))?;

    if rows.is_empty() {
        return Err(anyhow!("Table '{table}' not found or has no columns."));
    }

    let columns: Vec<Value> = rows.iter().map(row_to_json).collect();
    Ok(json!({ "table": table, "columns": columns }))
}

pub async fn get_full_schema(state: &SharedState) -> Result<Value> {
    let tables_val = list_tables(state).await?;
    let tables: Vec<String> = tables_val["tables"]
        .as_array()
        .unwrap_or(&vec![])
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();

    let mut schema = serde_json::Map::new();
    for table in &tables {
        match describe_table(state, table).await {
            Ok(info) => {
                schema.insert(table.clone(), info["columns"].clone());
            }
            Err(e) => {
                schema.insert(table.clone(), json!({ "error": e.to_string() }));
            }
        }
    }

    Ok(json!({ "schema": schema, "table_count": tables.len() }))
}

pub async fn get_db_info(state: &SharedState) -> Result<Value> {
    let st = state.lock().await;
    if !st.connected() {
        return Ok(json!({
            "connected": false,
            "message": "No active database connection."
        }));
    }
    Ok(json!({
        "connected": true,
        "db_type": st.kind().map(|k| k.label()).unwrap_or("unknown"),
        "connection": st.url.as_deref().map(redact_url).unwrap_or_default()
    }))
}


fn row_to_json(row: &sqlx::any::AnyRow) -> Value {
    let mut map = serde_json::Map::new();
    for col in row.columns() {
        let name = col.name().to_string();
        let type_name = col.type_info().name().to_lowercase();

        let val: Value = if type_name.contains("bool") {
            row.try_get::<Option<bool>, _>(col.ordinal())
                .ok()
                .flatten()
                .map(|v| json!(v))
                .unwrap_or(Value::Null)
        } else if type_name.contains("int")
            || type_name.contains("serial")
            || type_name.contains("bigint")
        {
            row.try_get::<Option<i64>, _>(col.ordinal())
                .ok()
                .flatten()
                .map(|v| json!(v))
                .unwrap_or(Value::Null)
        } else if type_name.contains("float")
            || type_name.contains("double")
            || type_name.contains("numeric")
            || type_name.contains("decimal")
            || type_name.contains("real")
        {
            row.try_get::<Option<f64>, _>(col.ordinal())
                .ok()
                .flatten()
                .map(|v| json!(v))
                .unwrap_or(Value::Null)
        } else {
            row.try_get::<Option<String>, _>(col.ordinal())
                .ok()
                .flatten()
                .map(|v| json!(v))
                .unwrap_or(Value::Null)
        };

        map.insert(name, val);
    }
    Value::Object(map)
}

fn redact_url(url: &str) -> String {
    if let Some(at) = url.rfind('@') {
        if let Some(slash2) = url.find("://") {
            let scheme_end = slash2 + 3;
            let creds = &url[scheme_end..at];
            if let Some(colon) = creds.find(':') {
                let user = &creds[..colon];
                return format!("{}://{}:****@{}", &url[..slash2], user, &url[at + 1..]);
            }
        }
    }
    url.to_string()
}
