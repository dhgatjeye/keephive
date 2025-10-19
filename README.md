# KeepHive

**Backup daemon for Windows**

[![Rust](https://img.shields.io/badge/rust-1.90.0-orange.svg)](https://blog.rust-lang.org/2025/09/18/Rust-1.90.0/)
[![Platform](https://img.shields.io/badge/platform-Windows-blue.svg)](https://www.microsoft.com/windows)

KeepHive is a backup daemon designed for Windows environments with a focus on ease of use. It runs as both a console application and a Windows Service, providing scheduled backups with automatic recovery and hot configuration reload.

---

## Quick Start

### 1. **Download**
```bash
# Clone the repository
git clone https://github.com/dhgatjeye/keephive.git
cd keephive

# Build release binary
cargo build --release
```

### 2. **Create Configuration**
Create `keephive_config.json`:
```json
{
  "jobs": [
    {
      "id": "documents_backup",
      "source": "C:\\Users\\user\\Documents",
      "target": "D:\\Backups\\Documents",
      "schedule": {
        "type": "daily",
        "hour": 2,
        "minute": 0
      },
      "description": "Daily backup of Documents at 2 AM"
    }
  ],
  "retention_count": 10,
  "log_level": "info",
  "state_path": ".keephive_state.json",
  "log_directory": "./logs",
  "log_rotation": {
    "type": "daily"
  }
}
```

### 3. **Run**

**Console Mode** (for testing):
```bash
keephive.exe keephive_config.json
```

**Windows Service** (for local):
```bash

## Run Terminal with Administrator

# Install service
keephive.exe --install C:\ProgramData\KeepHive\keephive_config.json

# Start service
keephive.exe --start

# Stop service
keephive.exe --stop

# Uninstall service
keephive.exe --uninstall
```

---

## 📖 Usage

### Command Line Options

```
USAGE:
  keephive.exe [CONFIG_FILE]              Run in console mode
  keephive.exe --install [CONFIG_FILE]    Install as Windows Service
  keephive.exe --uninstall                Uninstall Windows Service
  keephive.exe --start                    Start Windows Service
  keephive.exe --stop                     Stop Windows Service
  keephive.exe --help                     Show help
```

---

## ⚙️ Configuration

### Schedule Types

**Interval** - Run every N seconds:
```json
{
  "schedule": {
    "type": "interval",
    "seconds": 3600
  }
}
```

**Daily** - Run at specific time every day:
```json
{
  "schedule": {
    "type": "daily",
    "hour": 2,
    "minute": 30
  }
}
```

**Weekly** - Run on specific day of week:
```json
{
  "schedule": {
    "type": "weekly",
    "day": 7,
    "hour": 3,
    "minute": 0
  }
}
```
*Note: day 1 = Monday, 7 = Sunday*

### Log Rotation
Options: "daily", "hourly", "never"

```json
{
  "log_rotation": {
    "type": "daily"
  }
}
```

### Complete Configuration Example

```json
{
  "jobs": [
    {
      "id": "work_files",
      "source": "C:\\Work",
      "target": "D:\\Backups\\Work",
      "schedule": {
        "type": "interval",
        "seconds": 3600
      },
      "description": "Hourly backup of work files"
    },
    {
      "id": "photos",
      "source": "C:\\Users\\Me\\Pictures",
      "target": "D:\\Backup",
      "schedule": {
        "type": "weekly",
        "day": 7,
        "hour": 1,
        "minute": 0
      },
      "description": "Weekly photo backup on Sunday at 1 AM"
    }
  ],
  "retention_count": 5,
  "log_level": "info",
  "state_path": "C:\\ProgramData\\KeepHive\\keephive_state.json",
  "log_directory": "C:\\ProgramData\\KeepHive\\logs",
  "log_rotation": {
    "type": "daily"
  }
}
```

---

## File Locations

**Console Mode:**
- Config: `./keephive_config.json` (or specified path)
- State: `./keephive_state.json` (or as configured)
- Logs: `./logs` (or as configured)

**Service Mode:**
- Config: `C:\ProgramData\KeepHive\keephive_config.json` (recommended)
- State: Same directory as config (or as configured)
- Logs: Same directory as config (or as configured)

---


## 🛠️ Development

### Build from source
```bash
# Clean first
cargo clean

# Release build 
cargo build --release
```

### Project Structure
```
keephive/
├── src/
│   ├── main.rs              # Entry point
│   ├── lib.rs               # Library exports
│   ├── config/              # Configuration management
│   ├── core/                # Backup logic
│   ├── scheduler/           # Job scheduling
│   ├── state/               # State management
│   ├── service/             # Service daemon
│   ├── platform/windows/    # Windows-specific code
│   └── observability/       # Logging
└── Cargo.toml               # Dependencies
```