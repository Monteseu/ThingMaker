//! Ultra-Review records: one row per review, the whole review as JSON.

use rusqlite::{OptionalExtension, params};

use super::{Storage, StorageError};
use crate::ultrareview::model::{Review, Status};

fn status(status: Status) -> &'static str {
    match status {
        Status::Running => "running",
        Status::Done => "done",
        Status::Failed => "failed",
        Status::Cancelled => "cancelled",
        Status::Interrupted => "interrupted",
    }
}

fn read(value: String) -> Result<Review, StorageError> {
    serde_json::from_str(&value).map_err(|error| StorageError::Invalid(format!("an Ultra-Review record could not be read: {error}")))
}

impl Storage {
    pub fn ultra_review_save(&self, review: &Review) -> Result<(), StorageError> {
        let value = serde_json::to_string(review).map_err(|error| StorageError::Invalid(error.to_string()))?;
        self.conn().execute(
            "INSERT INTO ultra_reviews(id, workspace_id, host, status, value, created_at, updated_at) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7)
             ON CONFLICT(id) DO UPDATE SET status = excluded.status, value = excluded.value, updated_at = excluded.updated_at",
            params![review.id, review.workspace_id, review.host, status(review.status), value, review.created_at as i64, review.updated_at as i64],
        )?;
        Ok(())
    }

    pub fn ultra_review_get(&self, id: &str) -> Result<Option<Review>, StorageError> {
        let value: Option<String> = self.conn().query_row("SELECT value FROM ultra_reviews WHERE id = ?1", params![id], |row| row.get(0)).optional()?;
        value.map(read).transpose()
    }

    /// A session's reviews, newest first.
    pub fn ultra_review_list(&self, host: &str) -> Result<Vec<Review>, StorageError> {
        let mut statement = self.conn().prepare("SELECT value FROM ultra_reviews WHERE host = ?1 ORDER BY created_at DESC LIMIT 50")?;
        let values = statement.query_map(params![host], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        values.into_iter().map(read).collect()
    }

    /// Every finished review, for the reviewers' track record.
    pub fn ultra_review_finished(&self) -> Result<Vec<Review>, StorageError> {
        let mut statement = self.conn().prepare("SELECT value FROM ultra_reviews WHERE status = 'done' ORDER BY created_at DESC LIMIT 500")?;
        let values = statement.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        values.into_iter().map(read).collect()
    }

    /// Reviews that were running when ThingMaker last closed.
    pub fn ultra_review_running(&self) -> Result<Vec<Review>, StorageError> {
        let mut statement = self.conn().prepare("SELECT value FROM ultra_reviews WHERE status = 'running'")?;
        let values = statement.query_map([], |row| row.get::<_, String>(0))?.collect::<Result<Vec<_>, _>>()?;
        values.into_iter().map(read).collect()
    }

    pub fn ultra_review_delete(&self, id: &str) -> Result<(), StorageError> {
        self.conn().execute("DELETE FROM ultra_reviews WHERE id = ?1", params![id])?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ultrareview::debate::tests_support::review;

    #[test]
    fn a_review_is_kept_whole_and_listed_per_session() {
        let storage = Storage::open_in_memory().unwrap();
        let mut first = review(2);
        first.created_at = 1;
        storage.ultra_review_save(&first).unwrap();
        let mut second = review(3);
        second.id = "r2".into();
        second.created_at = 2;
        second.status = Status::Done;
        storage.ultra_review_save(&second).unwrap();
        first.round = 2;
        first.updated_at = 5;
        storage.ultra_review_save(&first).unwrap();
        assert_eq!(storage.ultra_review_get("r1").unwrap().unwrap().round, 2);
        assert_eq!(storage.ultra_review_list("s").unwrap().iter().map(|r| r.id.as_str()).collect::<Vec<_>>(), ["r2", "r1"]);
        assert_eq!(storage.ultra_review_running().unwrap().len(), 1);
        assert_eq!(storage.ultra_review_finished().unwrap()[0].id, "r2");
        assert!(storage.ultra_review_list("other").unwrap().is_empty());
        storage.ultra_review_delete("r1").unwrap();
        assert!(storage.ultra_review_get("r1").unwrap().is_none());
    }
}
