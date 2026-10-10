use std::collections::HashMap;
use std::path::Path;

use rusqlite::{Connection, params};

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum NameTarget {
    Node(String),
    Entity(String, u8),
    Channel(String, u8, u8),
}

pub struct Names {
    connection: Connection,
    names: HashMap<NameTarget, String>,
}

impl Names {
    pub fn open(path: impl AsRef<Path>) -> rusqlite::Result<Self> {
        Self::from_connection(Connection::open(path)?)
    }

    fn from_connection(connection: Connection) -> rusqlite::Result<Self> {
        connection.execute_batch(
            "CREATE TABLE IF NOT EXISTS node_names (
                device_id TEXT PRIMARY KEY,
                name TEXT NOT NULL
            );
            CREATE TABLE IF NOT EXISTS entity_names (
                device_id TEXT NOT NULL,
                entity INTEGER NOT NULL,
                name TEXT NOT NULL,
                PRIMARY KEY (device_id, entity)
            );
            CREATE TABLE IF NOT EXISTS channel_names (
                device_id TEXT NOT NULL,
                entity INTEGER NOT NULL,
                channel INTEGER NOT NULL,
                name TEXT NOT NULL,
                PRIMARY KEY (device_id, entity, channel)
            );",
        )?;

        let names = load(&connection)?;
        Ok(Self { connection, names })
    }

    pub fn get(&self, target: &NameTarget) -> Option<&str> {
        self.names.get(target).map(String::as_str)
    }

    /// An empty name removes it.
    pub fn set(&mut self, target: NameTarget, name: &str) -> rusqlite::Result<()> {
        let name = name.trim();
        if name.is_empty() {
            match &target {
                NameTarget::Node(device_id) => self
                    .connection
                    .execute("DELETE FROM node_names WHERE device_id = ?1", params![device_id])?,
                NameTarget::Entity(device_id, entity) => self.connection.execute(
                    "DELETE FROM entity_names WHERE device_id = ?1 AND entity = ?2",
                    params![device_id, entity],
                )?,
                NameTarget::Channel(device_id, entity, channel) => self.connection.execute(
                    "DELETE FROM channel_names WHERE device_id = ?1 AND entity = ?2 AND channel = ?3",
                    params![device_id, entity, channel],
                )?,
            };
            self.names.remove(&target);
            return Ok(());
        }

        match &target {
            NameTarget::Node(device_id) => self.connection.execute(
                "INSERT INTO node_names (device_id, name) VALUES (?1, ?2)
                 ON CONFLICT (device_id) DO UPDATE SET name = excluded.name",
                params![device_id, name],
            )?,
            NameTarget::Entity(device_id, entity) => self.connection.execute(
                "INSERT INTO entity_names (device_id, entity, name) VALUES (?1, ?2, ?3)
                 ON CONFLICT (device_id, entity) DO UPDATE SET name = excluded.name",
                params![device_id, entity, name],
            )?,
            NameTarget::Channel(device_id, entity, channel) => self.connection.execute(
                "INSERT INTO channel_names (device_id, entity, channel, name) VALUES (?1, ?2, ?3, ?4)
                 ON CONFLICT (device_id, entity, channel) DO UPDATE SET name = excluded.name",
                params![device_id, entity, channel, name],
            )?,
        };
        self.names.insert(target, name.to_string());
        Ok(())
    }
}

fn load(connection: &Connection) -> rusqlite::Result<HashMap<NameTarget, String>> {
    let mut names = HashMap::new();
    let mut statement = connection.prepare("SELECT device_id, name FROM node_names")?;
    for row in statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))? {
        let (device_id, name) = row?;
        names.insert(NameTarget::Node(device_id), name);
    }
    let mut statement = connection.prepare("SELECT device_id, entity, name FROM entity_names")?;
    for row in statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))? {
        let (device_id, entity, name) = row?;
        names.insert(NameTarget::Entity(device_id, entity), name);
    }
    let mut statement =
        connection.prepare("SELECT device_id, entity, channel, name FROM channel_names")?;
    for row in statement.query_map([], |row| {
        Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
    })? {
        let (device_id, entity, channel, name) = row?;
        names.insert(NameTarget::Channel(device_id, entity, channel), name);
    }
    Ok(names)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DEVICE: &str = "esp32-test";

    fn in_memory() -> Names {
        Names::from_connection(Connection::open_in_memory().unwrap()).unwrap()
    }

    #[test]
    fn set_and_get() {
        let mut names = in_memory();
        names
            .set(NameTarget::Node(DEVICE.into()), " Bedroom ")
            .unwrap();
        names
            .set(NameTarget::Entity(DEVICE.into(), 1), "Light")
            .unwrap();
        names
            .set(NameTarget::Channel(DEVICE.into(), 1, 0), "Power")
            .unwrap();

        assert_eq!(names.get(&NameTarget::Node(DEVICE.into())), Some("Bedroom"));
        assert_eq!(
            names.get(&NameTarget::Entity(DEVICE.into(), 1)),
            Some("Light")
        );
        assert_eq!(
            names.get(&NameTarget::Channel(DEVICE.into(), 1, 0)),
            Some("Power")
        );
        assert_eq!(names.get(&NameTarget::Entity(DEVICE.into(), 2)), None);
    }

    #[test]
    fn empty_name_removes_it() {
        let mut names = in_memory();
        names
            .set(NameTarget::Node(DEVICE.into()), "Bedroom")
            .unwrap();
        names.set(NameTarget::Node(DEVICE.into()), "  ").unwrap();
        assert_eq!(names.get(&NameTarget::Node(DEVICE.into())), None);
    }

    #[test]
    fn names_survive_reopening() {
        let path =
            std::env::temp_dir().join(format!("oxihome-names-{}.sqlite", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let mut names = Names::open(&path).unwrap();
        names
            .set(NameTarget::Channel(DEVICE.into(), 1, 0), "Power")
            .unwrap();
        names
            .set(NameTarget::Channel(DEVICE.into(), 1, 0), "Light power")
            .unwrap();
        drop(names);

        let names = Names::open(&path).unwrap();
        assert_eq!(
            names.get(&NameTarget::Channel(DEVICE.into(), 1, 0)),
            Some("Light power")
        );
        std::fs::remove_file(&path).unwrap();
    }
}
