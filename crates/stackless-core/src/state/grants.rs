//! Execution grants are written by the authenticated caller boundary.

use super::{StateError, Store};

impl Store {
    pub fn host_execution_allowed(&self, owner_id: &str) -> Result<bool, StateError> {
        Ok(self
            .query_row(
                "SELECT host_execution FROM execution_grants WHERE owner_id = ?1",
                &[owner_id.into()],
                |row| row.get_i64(0),
            )?
            .unwrap_or(0)
            == 1)
    }

    pub fn grant_host_execution(&self, owner_id: &str) -> Result<(), StateError> {
        let changed = self.execute(
            "INSERT INTO execution_grants(owner_id, host_execution, granted_at)
             SELECT ?1, 1, ?2 WHERE EXISTS(SELECT 1 FROM instances WHERE instance_id = ?1 AND status = 'active')
             ON CONFLICT(owner_id) DO UPDATE SET host_execution = 1, granted_at = excluded.granted_at",
            &[owner_id.into(), Self::now().into()],
        )?;
        if changed != 1 {
            return Err(StateError::ResourceInvariant {
                detail: "execution grant owner is not active".into(),
            });
        }
        Ok(())
    }
}
