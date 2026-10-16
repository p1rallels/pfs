//! Fork n owns a block of ports, computed from n alone. Nothing is stored.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub base: u32,
    pub size: u32,
    pub count: u32,
}

impl Default for Plan {
    /// 40000 + (n mod 900) × 10: below macos's ephemeral range (49152+),
    /// away from 3000/5000/7000.
    fn default() -> Self {
        Self {
            base: 40000,
            size: 10,
            count: 900,
        }
    }
}

impl Plan {
    pub fn is_valid(&self) -> bool {
        let top = u64::from(self.base) + u64::from(self.size) * u64::from(self.count);
        self.base >= 1024 && self.size > 0 && self.count > 0 && top <= 65536
    }

    pub fn block(&self, id: u32) -> (u32, u32) {
        let low = self.base + (id % self.count) * self.size;
        (low, low + self.size - 1)
    }

    pub fn base_port(&self, id: u32) -> u32 {
        self.block(id).0
    }
}

pub fn show((low, high): (u32, u32)) -> String {
    format!("{low}-{high}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_blocks() {
        let plan = Plan::default();
        assert_eq!(plan.block(5), (40050, 40059));
        assert_eq!(plan.block(3), (40030, 40039));
        assert_eq!(plan.block(905), plan.block(5));
        assert!(plan.block(899).1 < 49152);
        assert!(plan.is_valid());
    }

    #[test]
    fn rejects_overflowing_plans() {
        let plan = Plan {
            base: 60000,
            size: 10,
            count: 900,
        };
        assert!(!plan.is_valid());
    }
}
