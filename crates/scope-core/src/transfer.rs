//! Atomic resource-ownership handoff without double charging shared ancestors.
use crate::{Charge, Error, Kind, Registry, State, Usage, SYSTEM};
type Plan = ([usize; 6], [Usage; 6], usize);
impl Registry {
    fn transfer_plan(&self, source: &Charge, target: &Charge, used: Usage) -> Result<Plan, Error> {
        source.used.subtract(used)?;
        target.used.add(used)?;
        let (from, from_len) = self.lineage(source.scope)?;
        let (to, to_len) = self.lineage(target.scope)?;
        let mut indices = [0; 6];
        let mut totals = [Usage::ZERO; 6];
        let mut count = 0;
        for &index in from[..from_len].iter().chain(to[..to_len].iter()) {
            if indices[..count].contains(&index) {
                continue;
            }
            let node = self.slots[index].ok_or(Error::Missing)?;
            if node.state != State::Open {
                return Err(Error::Closed);
            }
            let mut total = node.used;
            if from[..from_len].contains(&index) {
                total = total.subtract(used)?;
            }
            if to[..to_len].contains(&index) {
                total = total.add(used)?;
            }
            let ceiling = if node.kind == Kind::System && target.scope != SYSTEM {
                node.limit.subtract(self.recovery)?
            } else {
                node.limit
            };
            // A system -> user grant is an admission into the non-system pool.
            // Conservatively refuse it while init is using the recovery floor.
            // Existing user -> user moves need no new common-ancestor capacity.
            let growth =
                if node.kind == Kind::System && source.scope == SYSTEM && target.scope != SYSTEM {
                    used
                } else {
                    total.increase_over(node.used)
                };
            if !total.permits_growth(growth, ceiling) {
                return Err(Error::Limit);
            }
            indices[count] = index;
            totals[count] = total;
            count += 1;
        }
        Ok((indices, totals, count))
    }
    /// Validate a native ownership move. The caller must also authenticate the
    /// objects/grant and hold its resource lock through validation and commit.
    pub fn can_transfer(&self, source: &Charge, target: &Charge, used: Usage) -> Result<(), Error> {
        self.transfer_plan(source, target, used).map(|_| ())
    }
    /// Move an existing resource charge. No actual allocation is implied. Every
    /// leaf/ancestor and both owners remain unchanged if validation fails.
    pub fn transfer(
        &mut self,
        source: &mut Charge,
        target: &mut Charge,
        used: Usage,
    ) -> Result<(), Error> {
        let (indices, totals, count) = self.transfer_plan(source, target, used)?;
        let remaining = source.used.subtract(used)?;
        let received = target.used.add(used)?;
        for n in 0..count {
            self.slots[indices[n]]
                .as_mut()
                .expect("validated transfer ancestor")
                .used = totals[n];
        }
        source.used = remaining;
        target.used = received;
        Ok(())
    }
}
