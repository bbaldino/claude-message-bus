//! Room naming. A DM is just a room with a derived name, so the rest of the
//! system has one concept to reason about even though the API has two.

use crate::proto::Target;

/// Members sorted, so both directions name the same room.
pub fn dm_name(a: &str, b: &str) -> String {
    let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
    format!("dm:{lo}|{hi}")
}

pub fn resolve(target: &Target, sender: &str) -> String {
    match target {
        Target::Room { room } => room.clone(),
        Target::Agent { name } => dm_name(sender, name),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proto::Target;

    #[test]
    fn dm_name_is_order_independent() {
        assert_eq!(dm_name("caas", "dashboard"), dm_name("dashboard", "caas"));
    }

    #[test]
    fn dm_name_has_the_documented_shape() {
        assert_eq!(dm_name("dashboard", "caas"), "dm:caas|dashboard");
    }

    #[test]
    fn a_room_target_resolves_to_itself() {
        let t = Target::Room {
            room: "protocol".into(),
        };
        assert_eq!(resolve(&t, "caas"), "protocol");
    }

    #[test]
    fn an_agent_target_resolves_to_the_pair_dm() {
        let t = Target::Agent {
            name: "dashboard".into(),
        };
        assert_eq!(resolve(&t, "caas"), "dm:caas|dashboard");
    }

    #[test]
    fn self_dm_is_stable() {
        let t = Target::Agent {
            name: "caas".into(),
        };
        assert_eq!(resolve(&t, "caas"), "dm:caas|caas");
    }
}
