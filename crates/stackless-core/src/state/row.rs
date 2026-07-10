//! One row of a result set, read positionally by mapping closures.

use super::error::StateError;
use super::value::Value;

/// A bounds-checked, panic-free view over either driver's row.
pub(super) struct Row {
    columns: Vec<Value>,
}

impl Row {
    pub(super) fn get_i64(&self, idx: usize) -> Result<i64, StateError> {
        match self.columns.get(idx) {
            Some(Value::Int(v)) => Ok(*v),
            Some(other) => Err(StateError::row_type(idx, &format!("i64, got {other:?}"))),
            None => Err(StateError::row_range(idx)),
        }
    }

    pub(super) fn get_u32(&self, idx: usize) -> Result<u32, StateError> {
        let v = self.get_i64(idx)?;
        u32::try_from(v).map_err(|_| StateError::row_type(idx, "u32"))
    }

    pub(super) fn get_string(&self, idx: usize) -> Result<String, StateError> {
        match self.columns.get(idx) {
            Some(Value::Text(t)) => Ok(t.clone()),
            Some(other) => Err(StateError::row_type(idx, &format!("text, got {other:?}"))),
            None => Err(StateError::row_range(idx)),
        }
    }

    /// A nullable integer column (`tombstoned_at`): NULL maps to `None`.
    pub(super) fn get_opt_i64(&self, idx: usize) -> Result<Option<i64>, StateError> {
        match self.columns.get(idx) {
            Some(Value::Null) => Ok(None),
            Some(Value::Int(v)) => Ok(Some(*v)),
            Some(other) => Err(StateError::row_type(
                idx,
                &format!("i64|null, got {other:?}"),
            )),
            None => Err(StateError::row_range(idx)),
        }
    }

    #[cfg(test)]
    pub(super) fn from_values(columns: Vec<Value>) -> Self {
        Self { columns }
    }

    pub(super) fn from_columns(columns: Vec<Value>) -> Self {
        Self { columns }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_i64_rejects_null_and_text() {
        let row = Row::from_values(vec![Value::Null, Value::Text("1".into())]);
        assert!(row.get_i64(0).is_err());
        assert!(row.get_i64(1).is_err());
    }

    #[test]
    fn get_string_rejects_null_and_int() {
        let row = Row::from_values(vec![Value::Null, Value::Int(7)]);
        assert!(row.get_string(0).is_err());
        assert!(row.get_string(1).is_err());
    }

    #[test]
    fn get_opt_i64_accepts_null_and_int_only() {
        let row = Row::from_values(vec![Value::Null, Value::Int(3), Value::Text("x".into())]);
        assert_eq!(row.get_opt_i64(0).unwrap(), None);
        assert_eq!(row.get_opt_i64(1).unwrap(), Some(3));
        assert!(row.get_opt_i64(2).is_err());
    }

    #[test]
    fn get_u32_rejects_negative() {
        let row = Row::from_values(vec![Value::Int(-1), Value::Int(42)]);
        assert!(row.get_u32(0).is_err());
        assert_eq!(row.get_u32(1).unwrap(), 42);
    }
}
