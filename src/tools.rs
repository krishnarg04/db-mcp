use std::sync::Arc;
use tokio::sync::Mutex;
use crate::db::{SharedState, ConfigSharedState};
use crate::protocol::{make_tool, str_prop, tool_err, tool_ok};
use serde_json::{json, Value};

pub fn tool_list() -> Value {
    json!({
        "tools": [
            make_tool(
                "connect_database",
                "Connect to a MySQL or PostgreSQL database. Provide either a connection_string URL, OR a saved_config_name to reconnect using credentials saved via configure_server. Must be called before any other database tool.",
                json!({
                    "connection_name":  str_prop("Optional label for this connection. Defaults to 'user@host'. Used to reference this connection in all other tools."),
                    "connection_string": str_prop(
                        "Database URL. MySQL: mysql://user:pass@host:3306/dbname  |  PostgreSQL: postgres://user:pass@host:5432/dbname. Required if saved_config_name is not provided."
                    ),
                    "saved_config_name": str_prop(
                        "Name of a previously saved connection (via configure_server). If provided, connection_string is not needed."
                    )
                }),
                &[],
            ),
            make_tool(
                "disconnect_database",
                "Close a database connection.",
                json!({
                    "connection_name": str_prop("Name of the connection to disconnect. If not provided, the first active connection will be disconnected.")
                }),
                &[],
            ),
            make_tool(
                "get_database_info",
                "Return info about a database connection (type, host, status).",
                json!({
                    "connection_name": str_prop("Name of the connection to get info for. If not provided, the first active connection is used.")
                }),
                &[],
            ),
            make_tool(
                "list_connections",
                "List all currently registered connection names.",
                json!({}),
                &[],
            ),
            make_tool(
                "list_databases",
                "List all databases / schemas visible to the current user.",
                json!({
                    "connection_name": str_prop("Name of the connection to use. If not provided, the first active connection is used.")
                }),
                &[],
            ),
            make_tool(
                "list_tables",
                "List all tables in the connected database.",
                json!({
                    "connection_name": str_prop("Name of the connection to use. If not provided, the first active connection is used.")
                }),
                &[],
            ),
            make_tool(
                "describe_table",
                "Return column definitions (name, type, nullability, default, key) for a given table. Use this before writing queries.",
                json!({
                    "connection_name": str_prop("Name of the connection to use. If not provided, the first active connection is used."),
                    "table_name": str_prop("The table to describe.")
                }),
                &["table_name"],
            ),
            make_tool(
                "get_full_schema",
                "Return the complete schema (every table + all columns). Call this before generating any SQL query.",
                json!({
                    "connection_name": str_prop("Name of the connection to use. If not provided, the first active connection is used.")
                }),
                &[],
            ),
            make_tool(
                "execute_query",
                "Execute a SQL query. SELECT/SHOW/EXPLAIN return rows as JSON. INSERT/UPDATE/DELETE return rows-affected count.",
                json!({
                    "connection_name": str_prop("Name of the connection to use. If not provided, the first active connection is used."),
                    "sql": str_prop("The SQL statement to execute.")
                }),
                &["sql"],
            ),
            make_tool(
                "configure_server",
                "Save connection details permanently to a config file. Use the saved name with connect_database to reconnect without providing credentials again.",
                json!({
                    "name":     str_prop("A name to identify this connection (e.g. 'prod-db')."),
                    "ip":       str_prop("Database server IP address or hostname."),
                    "port":     str_prop("Database server port (e.g. 3306 for MySQL, 5432 for PostgreSQL)."),
                    "username": str_prop("Username for database authentication."),
                    "password": str_prop("Password for database authentication."),
                    "dbtype":   str_prop("Type of database: 'mysql' or 'postgres'."),
                    "database": str_prop("Database / schema name to connect to. For PostgreSQL, defaults to the username if omitted.")
                }),
                &["name", "ip", "port", "username", "password", "dbtype"],
            ),
        ]
    })
}


fn resolve_state_for_name(config: &crate::db::ConfigVsDBstate, name_opt: Option<&str>,) -> Result<SharedState, String> {
    match name_opt {
        Some(name) => {
            config.get(name).ok_or_else(|| {
                format!(
                    "No connection found with name '{name}'. \
                     Available connections: [{}]. \
                     Use connect_database to open a new connection.",
                    config.names().join(", ")
                )
            })
        }
        None => {
            config.get_first().ok_or_else(|| {
                "No connection_name provided and no active connections found. \
                 Use connect_database first."
                    .to_string()
            })
        }
    }
}

pub async fn dispatch(tool: &str, args: &Value, state: &ConfigSharedState) -> Value {
    match tool {
        "connect_database" => {
            let url = if let Some(u) = args.get("connection_string").and_then(|v| v.as_str()) {
                u.to_string()
            } else if let Some(saved_name) = args.get("saved_config_name").and_then(|v| v.as_str()) {
                match crate::config::get_connection_url(saved_name) {
                    Some(url) => url,
                    None => return tool_err(format!(
                        "No saved connection found with name '{saved_name}'. \
                         Use configure_server to save one first."
                    )),
                }
            } else {
                return tool_err(
                    "Provide either 'connection_string' (URL) or 'saved_config_name' (a name saved via configure_server)."
                );
            };

            let dbtype = if url.starts_with("mysql://") || url.starts_with("mariadb://") {
                "mysql"
            } else if url.starts_with("postgres://") || url.starts_with("postgresql://") {
                "postgres"
            } else {
                return tool_err(
                    "Invalid connection string. Must start with mysql:// or postgres://",
                );
            };

            let host = url
                .split('@')
                .nth(1)
                .and_then(|h| h.split(':').next())
                .unwrap_or("")
                .to_string();
            let port: u16 = url
                .split('@')
                .nth(1)
                .and_then(|h| h.split(':').nth(1))
                .and_then(|p| p.split('/').next())
                .and_then(|p| p.parse().ok())
                .unwrap_or(0);
            let username = url
                .split("://")
                .nth(1)
                .and_then(|u| u.split(':').next())
                .unwrap_or("")
                .to_string();
            let password = url
                .split("://")
                .nth(1)
                .and_then(|rest| rest.split('@').next())
                .and_then(|creds| creds.split(':').nth(1))
                .unwrap_or("")
                .to_string();

            let database = url
                .split('@')
                .nth(1)
                .and_then(|h| h.splitn(2, '/').nth(1))
                .unwrap_or("")
                .to_string();

            let conn_name = args
                .get("connection_name")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .unwrap_or_else(|| format!("{username}@{host}"));

            let new_db_state: SharedState = Arc::new(Mutex::new(crate::db::DbState::new()));
            let connect_msg = match crate::db::connect(&new_db_state, &url).await {
                Ok(msg) => msg,
                Err(e) => return tool_err(format!("Error {e}")),
            };

            {
                let mut cfg = state.lock().await;
                cfg.add(conn_name.clone(), new_db_state);
            }

            if let Err(e) = crate::config::add_temporary_entry(conn_name.clone(),host,port,username,password,dbtype.to_string(),database,) 
            {
                return tool_err(format!(" Config error: {e}"));
            }

            tool_ok(format!("{connect_msg}\nRegistered as '{conn_name}'."))
        }

        "disconnect_database" => {
            let conn_name = args.get("connection_name").and_then(|v| v.as_str());

            let db_state = {
                let mut cfg = state.lock().await;
                match conn_name {
                    Some(name) => match cfg.remove(name) {
                        Some(s) => s,
                        None => {
                            return tool_err(format!(
                                "No connection found with name '{name}'. \
                                 Available: [{}]",
                                cfg.names().join(", ")
                            ))
                        }
                    },
                    None => match cfg.get_first() {
                        Some(s) => {
                            let first_name = cfg
                                .names()
                                .into_iter()
                                .next()
                                .unwrap_or_default();
                            cfg.remove(&first_name);
                            s
                        }
                        None => {
                            return tool_err(
                                "No active connections to disconnect.",
                            )
                        }
                    },
                }
            };

            match crate::db::disconnect(&db_state).await {
                Ok(msg) => tool_ok(msg),
                Err(e) => tool_err(format!("❌ {e}")),
            }
        }

        "get_database_info" => {
            let conn_name = args.get("connection_name").and_then(|v| v.as_str());
            let db_state = {
                let cfg = state.lock().await;
                match resolve_state_for_name(&cfg, conn_name) {
                    Ok(s) => s,
                    Err(e) => return tool_err(e),
                }
            };
            match crate::db::get_db_info(&db_state).await {
                Ok(v) => tool_ok(serde_json::to_string_pretty(&v).unwrap_or_default()),
                Err(e) => tool_err(format!("Error {e}")),
            }
        }

        "list_connections" => {
            let cfg = state.lock().await;
            let names = cfg.names();
            if names.is_empty() {
                tool_ok("No active connections.")
            } else {
                tool_ok(
                    serde_json::to_string_pretty(&json!({ "connections": names }))
                        .unwrap_or_default(),
                )
            }
        }

        "list_databases" => {
            let conn_name = args.get("connection_name").and_then(|v| v.as_str());
            let db_state = {
                let cfg = state.lock().await;
                match resolve_state_for_name(&cfg, conn_name) {
                    Ok(s) => s,
                    Err(e) => return tool_err(e),
                }
            };
            match crate::db::list_databases(&db_state).await {
                Ok(v) => tool_ok(serde_json::to_string_pretty(&v).unwrap_or_default()),
                Err(e) => tool_err(format!("Error {e}")),
            }
        }

        "list_tables" => {
            let conn_name = args.get("connection_name").and_then(|v| v.as_str());
            let db_state = {
                let cfg = state.lock().await;
                match resolve_state_for_name(&cfg, conn_name) {
                    Ok(s) => s,
                    Err(e) => return tool_err(e),
                }
            };
            match crate::db::list_tables(&db_state).await {
                Ok(v) => tool_ok(serde_json::to_string_pretty(&v).unwrap_or_default()),
                Err(e) => tool_err(format!("error {e}")),
            }
        }

        "describe_table" => {
            let table = match args.get("table_name").and_then(|v| v.as_str()) {
                Some(t) => t.to_string(),
                None => return tool_err("Missing required argument: table_name"),
            };
            let conn_name = args.get("connection_name").and_then(|v| v.as_str());
            let db_state = {
                let cfg = state.lock().await;
                match resolve_state_for_name(&cfg, conn_name) {
                    Ok(s) => s,
                    Err(e) => return tool_err(e),
                }
            };
            match crate::db::describe_table(&db_state, &table).await {
                Ok(v) => tool_ok(serde_json::to_string_pretty(&v).unwrap_or_default()),
                Err(e) => tool_err(format!("Error {e}")),
            }
        }

        "get_full_schema" => {
            let conn_name = args.get("connection_name").and_then(|v| v.as_str());
            let db_state = {
                let cfg = state.lock().await;
                match resolve_state_for_name(&cfg, conn_name) {
                    Ok(s) => s,
                    Err(e) => return tool_err(e),
                }
            };
            match crate::db::get_full_schema(&db_state).await {
                Ok(v) => tool_ok(serde_json::to_string_pretty(&v).unwrap_or_default()),
                Err(e) => tool_err(format!("Error {e}")),
            }
        }

        "execute_query" => {
            let sql = match args.get("sql").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return tool_err("Missing required argument: sql"),
            };
            let conn_name = args.get("connection_name").and_then(|v| v.as_str());
            let db_state = {
                let cfg = state.lock().await;
                match resolve_state_for_name(&cfg, conn_name) {
                    Ok(s) => s,
                    Err(e) => return tool_err(e),
                }
            };
            match crate::db::execute_query(&db_state, &sql).await {
                Ok(v) => tool_ok(serde_json::to_string_pretty(&v).unwrap_or_default()),
                Err(e) => tool_err(format!("error {e}")),
            }
        }

        "configure_server" => {
            let name = match args.get("name").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return tool_err("Missing required argument: name"),
            };
            let ip = match args.get("ip").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return tool_err("Missing required argument: ip"),
            };
            let port: u16 = match args.get("port").and_then(|v| v.as_str()) {
                Some(s) => match s.parse() {
                    Ok(p) => p,
                    Err(_) => return tool_err("Argument 'port' must be a valid number (e.g. \"5432\")"),
                },
                None => return tool_err("Missing required argument: port"),
            };
            let username = match args.get("username").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return tool_err("Missing required argument: username"),
            };
            let password = match args.get("password").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return tool_err("Missing required argument: password"),
            };
            let dbtype = match args.get("dbtype").and_then(|v| v.as_str()) {
                Some(s) => s.to_string(),
                None => return tool_err("Missing required argument: dbtype"),
            };
            let database = args
                .get("database")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            match crate::config::add_permanent_entry(
                name, ip, port, username, password, dbtype, database,
            ) {
                Ok(msg) => tool_ok(msg),
                Err(e) => tool_err(format!("Error {e}")),
            }
        }

        other => tool_err(format!("Unknown tool: '{other}'")),
    }
}