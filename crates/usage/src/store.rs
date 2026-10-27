use chrono::{DateTime, Duration, Local, Utc};
use rusqlite::{params, Connection};
use std::path::Path;

use crate::event::UsageEvent;
use crate::pricing::PricingConfig;

#[derive(Debug, Clone, Default)]
pub struct UsageAggregate {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read: u64,
    pub cache_write: u64,
    pub estimated_cost_usd: f64,
    pub event_count: usize,
}

pub struct UsageStore {
    conn: Connection,
    pricing: PricingConfig,
}

impl UsageStore {
    pub fn open(path: impl AsRef<Path>) -> Result<Self, rusqlite::Error> {
        let conn = Connection::open(path)?;
        let mut store = Self {
            conn,
            pricing: PricingConfig::default(),
        };
        store.init_schema()?;
        Ok(store)
    }

    pub fn open_in_memory() -> Result<Self, rusqlite::Error> {
        let conn = Connection::open_in_memory()?;
        let mut store = Self {
            conn,
            pricing: PricingConfig::default(),
        };
        store.init_schema()?;
        Ok(store)
    }

    fn init_schema(&mut self) -> Result<(), rusqlite::Error> {
        self.conn.execute(
            "CREATE TABLE IF NOT EXISTS usage_events (
                id INTEGER PRIMARY KEY AUTOINCREMENT,
                agent TEXT NOT NULL,
                session_id TEXT NOT NULL,
                step_id TEXT,
                project TEXT,
                model TEXT NOT NULL,
                input_tokens INTEGER NOT NULL,
                output_tokens INTEGER NOT NULL,
                cache_read INTEGER NOT NULL,
                cache_write INTEGER NOT NULL,
                cost_usd REAL NOT NULL,
                timestamp TEXT NOT NULL,
                UNIQUE(agent, session_id, step_id)
            )",
            [],
        )?;
        Ok(())
    }

    pub fn insert_event(&mut self, event: &UsageEvent) -> Result<bool, rusqlite::Error> {
        let cost = self.pricing.estimate_cost(event);
        let ts_str = event.timestamp.to_rfc3339();

        let res = self.conn.execute(
            "INSERT OR IGNORE INTO usage_events (
                agent, session_id, step_id, project, model,
                input_tokens, output_tokens, cache_read, cache_write,
                cost_usd, timestamp
            ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            params![
                event.agent,
                event.session_id,
                event.step_id,
                event.project,
                event.model,
                event.input_tokens,
                event.output_tokens,
                event.cache_read,
                event.cache_write,
                cost,
                ts_str,
            ],
        )?;

        Ok(res > 0)
    }

    pub fn insert_events_batch(&mut self, events: &[UsageEvent]) -> Result<usize, rusqlite::Error> {
        if events.is_empty() {
            return Ok(0);
        }
        let pricing = self.pricing.clone();
        let tx = self.conn.transaction()?;
        let mut inserted_count = 0;
        {
            let mut stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO usage_events (
                    agent, session_id, step_id, project, model,
                    input_tokens, output_tokens, cache_read, cache_write,
                    cost_usd, timestamp
                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
            )?;
            for event in events {
                let cost = pricing.estimate_cost(event);
                let ts_str = event.timestamp.to_rfc3339();
                let res = stmt.execute(params![
                    event.agent,
                    event.session_id,
                    event.step_id,
                    event.project,
                    event.model,
                    event.input_tokens,
                    event.output_tokens,
                    event.cache_read,
                    event.cache_write,
                    cost,
                    ts_str,
                ])?;
                if res > 0 {
                    inserted_count += res;
                }
            }
        }
        tx.commit()?;
        Ok(inserted_count)
    }

    pub fn query_aggregate_since(
        &self,
        since: DateTime<Utc>,
    ) -> Result<UsageAggregate, rusqlite::Error> {
        let since_str = since.to_rfc3339();
        let mut stmt = self.conn.prepare(
            "SELECT 
                COALESCE(SUM(input_tokens), 0),
                COALESCE(SUM(output_tokens), 0),
                COALESCE(SUM(cache_read), 0),
                COALESCE(SUM(cache_write), 0),
                COALESCE(SUM(cost_usd), 0.0),
                COUNT(id)
            FROM usage_events WHERE timestamp >= ?1",
        )?;

        let row = stmt.query_row(params![since_str], |r| {
            Ok(UsageAggregate {
                input_tokens: r.get(0)?,
                output_tokens: r.get(1)?,
                cache_read: r.get(2)?,
                cache_write: r.get(3)?,
                estimated_cost_usd: r.get(4)?,
                event_count: r.get(5)?,
            })
        })?;

        Ok(row)
    }

    /// Usage since local midnight (the user's "today", not UTC's).
    pub fn query_today(&self) -> Result<UsageAggregate, rusqlite::Error> {
        let today_start = Local::now()
            .date_naive()
            .and_hms_opt(0, 0, 0)
            .and_then(|midnight| midnight.and_local_timezone(Local).earliest())
            .map(|dt| dt.with_timezone(&Utc))
            .unwrap_or_else(|| Utc::now() - Duration::hours(24));
        self.query_aggregate_since(today_start)
    }

    pub fn query_rolling_5h(&self) -> Result<UsageAggregate, rusqlite::Error> {
        let five_hours_ago = Utc::now() - Duration::hours(5);
        self.query_aggregate_since(five_hours_ago)
    }
}
