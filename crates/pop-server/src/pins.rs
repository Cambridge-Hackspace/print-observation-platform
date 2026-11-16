//! Bambu certificate pins, kept in the database (#31).
//!
//! The driver asks for pins synchronously, so they are held in memory,
//! loaded once at start, and every new pin is written through. A pin whose
//! write has not landed when POP stops is trusted afresh on the next start;
//! a refused certificate never is.

use std::{collections::HashMap, sync::Mutex};

use pop_printers::bambu::PinStore;
use sqlx::SqlitePool;

use crate::users::now;

pub struct DbPins {
    db: SqlitePool,
    cache: Mutex<HashMap<String, String>>,
}

impl DbPins {
    /// Every pin the database holds.
    pub async fn load(db: SqlitePool) -> Result<Self, sqlx::Error> {
        let rows: Vec<(String, String)> =
            sqlx::query_as("SELECT serial, fingerprint FROM bambu_pins")
                .fetch_all(&db)
                .await?;
        Ok(Self {
            db,
            cache: Mutex::new(rows.into_iter().collect()),
        })
    }
}

impl PinStore for DbPins {
    fn pinned(&self, serial: &str) -> Option<String> {
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(serial)
            .cloned()
    }

    fn pin(&self, serial: &str, fingerprint: &str) {
        self.cache
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(serial.to_owned(), fingerprint.to_owned());
        let db = self.db.clone();
        let (serial, fingerprint) = (serial.to_owned(), fingerprint.to_owned());
        tokio::spawn(async move {
            let r = sqlx::query(
                "INSERT INTO bambu_pins (serial, fingerprint, pinned_at) VALUES (?, ?, ?)
                 ON CONFLICT (serial) DO UPDATE SET fingerprint = excluded.fingerprint,
                                                    pinned_at = excluded.pinned_at",
            )
            .bind(&serial)
            .bind(&fingerprint)
            .bind(now())
            .execute(&db)
            .await;
            if let Err(e) = r {
                tracing::error!("keeping printer {serial}'s certificate pin: {e}");
            }
        });
    }
}
