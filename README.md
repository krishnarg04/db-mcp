# db-mcp

A lightweight **Model Context Protocol (MCP)** server written in Rust that connects **MySQL** and **PostgreSQL** databases to LLMs inside editors like **VS Code** and **Zed**.

---

## Table of Contents

- [Overview](#overview)
- [Features](#features)
- [Prerequisites](#prerequisites)
- [Installation](#installation)
- [Configuration](#configuration)
  - [VS Code](#vs-code)
  - [Zed](#zed)
  - [Qwen CLI](#qwen-cli)
  - [Gemini CLI](#gemini-cli)
- [Available Tools](#available-tools)
- [Persistent Connections](#persistent-connections)
  - [Save a connection](#save-a-connection)
  - [Reconnect by name](#reconnect-by-name)
  - [Config file format](#config-file-format)
  - [Config file location](#config-file-location)
  - [Editing the config file manually](#editing-the-config-file-manually)
- [Usage Examples](#usage-examples)
- [Project Structure](#project-structure)
- [Building from Source](#building-from-source)
- [Environment Variables](#environment-variables)
- [Connection String Format](#connection-string-format)
- [License](#license)

---

## Overview

`db-mcp` implements the [Model Context Protocol](https://modelcontextprotocol.io/) over **stdio** (newline-delimited JSON-RPC 2.0). When registered as an MCP server in your editor, it exposes a set of database tools that your AI assistant can call to:

- Connect / disconnect from one or more databases simultaneously
- Save connection credentials persistently and reconnect by name
- Inspect schemas and table definitions
- Execute arbitrary SQL queries and get results back as structured JSON

---

## Features

- **Multi-database support** — MySQL, MariaDB, and PostgreSQL via `sqlx`
- **Multiple simultaneous connections** — open several databases at once and switch between them by name
- **Persistent connections** — save credentials with `configure_server`, reconnect instantly with just a name
- **MCP-compliant** — works with any editor or agent that speaks the Model Context Protocol
- **Schema introspection** — list databases, list tables, describe individual tables, or dump the entire schema at once
- **Safe credential redaction** — passwords are masked in all log output and connection-info responses
- **Async & lightweight** — built on Tokio; single binary with no runtime dependencies
- **Static musl binary available** — copy to any Linux machine or Docker container and run without installing anything

---

## Prerequisites

| Tool | Version |
|------|---------|
| Rust | 1.75 or later |
| Cargo | ships with Rust |
| A running MySQL / MariaDB **or** PostgreSQL instance | any recent version |

---

## Installation

### From source (glibc — default)

```sh
git clone https://github.com/your-org/db-mcp.git
cd db-mcp
cargo build --release
# Binary is at ./target/release/db-mcp
```

### From source (fully static musl binary — recommended for distribution)

```sh
# Add the musl target once
rustup target add x86_64-unknown-linux-musl

# Install the musl linker (Debian / Ubuntu)
sudo apt-get install -y musl-tools

# Build
cargo build --release --target x86_64-unknown-linux-musl
# Binary is at ./target/x86_64-unknown-linux-musl/release/db-mcp

# Verify it is fully static
ldd ./target/x86_64-unknown-linux-musl/release/db-mcp
# → statically linked
```

The musl binary has **zero runtime dependencies** — no glibc, no OpenSSL, no system libraries. Copy it to any x86-64 Linux machine or Docker container and run it directly.

### Install to PATH

```sh
cargo install --path .
```

---

## Configuration

Register `db-mcp` as an MCP server in your editor so that it is launched automatically when the editor starts.

### VS Code

Add the following to your `settings.json` (or to the workspace `.vscode/settings.json`):

```json
{
  "mcp.servers": {
    "db-mcp": {
      "command": "/absolute/path/to/db-mcp",
      "args": [],
      "transport": "stdio"
    }
  }
}
```

### Zed

Add the following to your `~/.config/zed/settings.json`:

```json
{
  "context_servers": {
    "db-mcp": {
      "command": {
        "path": "/absolute/path/to/db-mcp",
        "args": []
      }
    }
  }
}
```

### Qwen CLI

Qwen CLI reads MCP server configuration from `~/.qwen/settings.json`. Add the following block:

```json
{
  "mcpServers": {
    "db-mcp": {
      "command": "/absolute/path/to/db-mcp",
      "args": [],
      "transport": "stdio"
    }
  }
}
```

If the file does not exist yet, create it:

```sh
mkdir -p ~/.qwen
touch ~/.qwen/settings.json
```

Then paste the JSON above into the file. On the next `qwen` invocation the `db-mcp` tools will be available automatically.

> **Tip:** You can verify the server is loaded by running `qwen mcp list` — `db-mcp` should appear in the output.

### Gemini CLI

Gemini CLI reads MCP server configuration from `~/.gemini/settings.json`. Add the following block:

```json
{
  "mcpServers": {
    "db-mcp": {
      "command": "/absolute/path/to/db-mcp",
      "args": [],
      "transport": "stdio"
    }
  }
}
```

If the file does not exist yet, create it:

```sh
mkdir -p ~/.gemini
touch ~/.gemini/settings.json
```

Then paste the JSON above into the file. Gemini CLI will launch `db-mcp` automatically as a subprocess and communicate with it over stdio.

> **Tip:** You can verify the server is loaded by running `gemini mcp list` — `db-mcp` should appear in the output.

---

## Available Tools

Once registered, the following tools are exposed to the LLM:

| Tool | Required args | Description |
|------|--------------|-------------|
| `connect_database` | `connection_string` **or** `saved_config_name` | Open a connection. Optionally label it with `connection_name`. |
| `disconnect_database` | — | Close a connection by `connection_name`, or the first active one. |
| `get_database_info` | — | Return type, host, and status for a connection. |
| `list_connections` | — | List all currently open connection names. |
| `list_databases` | — | List all databases / schemas visible to the connected user. |
| `list_tables` | — | List all tables in the connected database. |
| `describe_table` | `table_name` | Return column definitions (name, type, nullability, default, key). |
| `get_full_schema` | — | Dump the complete schema — every table and all its columns. |
| `execute_query` | `sql` | Run any SQL. `SELECT`/`SHOW`/`EXPLAIN` → JSON rows; `INSERT`/`UPDATE`/`DELETE` → rows-affected count. |
| `configure_server` | `name`, `ip`, `port`, `username`, `password`, `dbtype` | **Save** connection details to `~/.db-mcp/config.json` for future use. |

> All tools that operate on a connection accept an optional `connection_name` argument.
> If omitted, the first open connection is used automatically.

---

## Persistent Connections

`db-mcp` can save your database credentials to a config file so you never have to type them again. Use `configure_server` once, then reconnect in any future session with just a name.

### Save a connection

Call `configure_server` with the connection details:

```
configure_server(
  name     = "pgdb",
  ip       = "127.0.0.1",
  port     = "5432",
  username = "user",
  password = "secret",
  dbtype   = "postgres",
  database = "pgdb"          ← optional; defaults to username if omitted
)
```

The credentials are written to `~/.db-mcp/config.json` immediately. The entry survives restarts — `db-mcp` loads the file automatically every time it starts.

### Reconnect by name

In any later session, connect with just the saved name — no URL or credentials needed:

```
connect_database(saved_config_name = "pgdb")

# Give it a custom label for this session (optional)
connect_database(saved_config_name = "pgdb", connection_name = "production")
```

### Config file format

The file is **newline-delimited JSON** (one compact JSON object per line). Each line represents one saved connection:

```json
{"name":"pgdb","ip":"127.0.0.1","port":5432,"username":"root","password":"secret","dbtype":"postgres","database":"sasdb"}
{"name":"local-mysql","ip":"127.0.0.1","port":3306,"username":"dev","password":"devpass","dbtype":"mysql","database":"appdb"}
```

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Unique identifier used to reconnect |
| `ip` | string | Hostname or IP address of the database server |
| `port` | number | Port number (`5432` for PostgreSQL, `3306` for MySQL) |
| `username` | string | Database username |
| `password` | string | Database password |
| `dbtype` | string | `"postgres"` or `"mysql"` |
| `database` | string | Target database / schema. For PostgreSQL, defaults to `username` if blank |

> **PostgreSQL default database:** If `database` is left empty, PostgreSQL connects to a database
> with the **same name as the username** (standard libpq behaviour). Always specify `database`
> explicitly to avoid surprises.

### Config file location

| Platform | Path |
|----------|------|
| Linux / macOS | `~/.db-mcp/config.json` |
| Fallback (no `$HOME`) | `./db_config.json` (current working directory) |

The parent directory `~/.db-mcp/` is created automatically on first write.

### Editing the config file manually

You can add, edit, or remove entries directly in the file with any text editor. Rules to follow:

- **One JSON object per line** — do not use pretty-printed / multi-line JSON.
- All seven fields should be present; `database` may be an empty string `""`.
- Duplicate `name` values: the last one in the file wins on load.
- Deleting a line removes that saved connection permanently.

Example — add a new entry manually:

```sh
echo '{"name":"staging","ip":"192.168.1.50","port":5432,"username":"app","password":"apppass","dbtype":"postgres","database":"stagingdb"}' \
  >> ~/.db-mcp/config.json
```

---

## Usage Examples

### Connect with a URL

```
connect_database(connection_string = "postgres://alice:secret@localhost:5432/mydb")
```

### Connect with a URL and give it a label

```
connect_database(
  connection_string = "mysql://root:pass@127.0.0.1:3306/shop",
  connection_name   = "local-shop"
)
```

### Save credentials for later

```
configure_server(
  name     = "pgdevdb",
  ip       = "localhost",
  port     = "5432",
  username = "postgres",
  password = "postgres",
  dbtype   = "postgres",
  database = "myapp"
)
```

### Reconnect using a saved name

```
connect_database(saved_config_name = "pgdevdb")
```

### Work with multiple connections at once

```
connect_database(saved_config_name = "pgdevdb",    connection_name = "dev")
connect_database(saved_config_name = "pgdb",   connection_name = "prod")

list_connections()
# → { "connections": ["dev", "prod"] }

list_tables(connection_name = "dev")
execute_query(connection_name = "prod", sql = "SELECT COUNT(*) FROM orders")
```

### List all tables

```
list_tables()
```

### Describe a table

```
describe_table(table_name = "orders")
```

### Run a query

```
execute_query(sql = "SELECT id, name, created_at FROM users WHERE active = true LIMIT 20")
```

### Disconnect a specific connection

```
disconnect_database(connection_name = "prod")
```

---

## Project Structure

```
db-mcp/
├── Cargo.toml          # Package manifest & dependencies
├── Cargo.lock
└── src/
    ├── main.rs         # Entry point — JSON-RPC 2.0 stdio loop
    ├── db.rs           # Multi-connection state, db operations, schema introspection
    ├── tools.rs        # MCP tool definitions (list) and dispatch logic
    ├── protocol.rs     # JSON-RPC & MCP protocol types and helpers
    └── config.rs       # Persistent connection config — load/save ~/.db-mcp/config.json
```

### Module responsibilities

- **`main.rs`** — reads newline-delimited JSON-RPC from stdin, dispatches to handlers, writes responses to stdout. Handles `initialize`, `ping`, `tools/list`, and `tools/call` MCP methods.
- **`db.rs`** — owns `ConfigVsDBstate` (a `HashMap<name → SharedState>`) and `DbState` (pool + db kind + URL per connection). Implements all async database operations via `sqlx::AnyPool` so the same code path works for both MySQL and PostgreSQL.
- **`tools.rs`** — declares the MCP tool manifest returned to the client, implements `resolve_state_for_name` to look up a connection by optional name, and routes every `tools/call` request to the correct `db.rs` function.
- **`protocol.rs`** — lightweight JSON-RPC 2.0 request / response structs and MCP helper builders (`tool_ok`, `tool_err`, `make_tool`, `str_prop`).
- **`config.rs`** — loads `~/.db-mcp/config.json` on startup, exposes `configure_server` (persist) and `add_temporary_entry` (session-only), and provides `get_connection_url` to reconstruct a connection URL from a saved entry.

---

## Building from Source

```sh
# Debug build (faster compile, slower binary)
cargo build

# Release build (optimised)
cargo build --release

# Fully static musl build (runs on any x86-64 Linux, no dependencies)
cargo build --release --target x86_64-unknown-linux-musl

# Run directly
cargo run

# Run with verbose logging
RUST_LOG=db_mcp=debug cargo run
```

### One-time musl setup

```sh
rustup target add x86_64-unknown-linux-musl   # add Rust target
sudo apt-get install -y musl-tools             # install musl-gcc linker
```

### Binary locations

| Build | Path |
|-------|------|
| Debug | `target/debug/db-mcp` |
| Release (glibc) | `target/release/db-mcp` |
| Release (musl, static) | `target/x86_64-unknown-linux-musl/release/db-mcp` |

---

## Environment Variables

| Variable | Default | Description |
|----------|---------|-------------|
| `RUST_LOG` | `db_mcp=info` | Log level filter. Logs are written to **stderr** so they never pollute the MCP stdio channel. Example: `RUST_LOG=db_mcp=debug` |

---

## Connection String Format

```
# PostgreSQL
postgres://username:password@host:5432/database

# MySQL / MariaDB
mysql://username:password@host:3306/database
```

> **PostgreSQL default database:** Omitting the database segment (`postgres://user:pass@host:5432/`)
> causes PostgreSQL to connect to a database with the **same name as the username**.
> Always include the database name to be explicit.

> **Security note:** Connection strings contain credentials. Avoid committing them to version
> control. Use `configure_server` to store them in `~/.db-mcp/config.json` instead, or pass
> them at runtime through your editor's MCP server configuration.

---

## Dependencies

| Crate | Purpose |
|-------|---------|
| `tokio` | Async runtime |
| `serde` / `serde_json` | JSON serialisation |
| `sqlx` | Async database driver (MySQL + PostgreSQL) |
| `anyhow` | Ergonomic error handling |
| `tracing` / `tracing-subscriber` | Structured logging to stderr |

---
