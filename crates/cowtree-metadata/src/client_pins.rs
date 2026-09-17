// Copyright (c) 2026 Windsor Nguyen

//! Durable object roots owned by the serialized filesystem adapter.

use std::collections::BTreeSet;

use rusqlite::TransactionBehavior;

use crate::{Result, Store, objects::ObjectId};

impl Store {
    /// Atomically replace the adapter's complete set of still-needed origin objects.
    ///
    /// The adapter must serialize this with its filesystem journals and retain prior
    /// roots until replacement records are durable. Construction uploads protect new
    /// objects until they become client roots. A rejected or interrupted transaction
    /// leaves the previous roots intact; an acknowledged replacement survives restart.
    pub fn replace_client_pins(&mut self, objects: &BTreeSet<ObjectId>) -> Result<()> {
        let tx = self.connection.transaction_with_behavior(TransactionBehavior::Immediate)?;
        let previous = {
            let mut statement = tx.prepare("SELECT object FROM client_pins")?;
            let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
            rows.map(|row| Ok(ObjectId::parse(&row?)?)).collect::<Result<BTreeSet<_>>>()?
        };
        for object in objects.difference(&previous) {
            self.objects.read(object)?;
            tx.execute("INSERT INTO client_pins(object) VALUES(?1)", [object.as_str()])?;
        }
        for object in previous.difference(objects) {
            tx.execute("DELETE FROM client_pins WHERE object=?1", [object.as_str()])?;
        }
        crate::fault::checkpoint("before-client-pins-commit");
        tx.commit()?;
        crate::fault::checkpoint("after-client-pins-commit");
        Ok(())
    }
}
