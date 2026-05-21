//! The viewer access code (a.k.a. the "password" the operator shares).
//!
//! The code is generated ONCE per process in `main()`, stored on `HostConfig`,
//! and is the single source of truth for both the GUI display and the
//! `session.json` we serve to the browser. The browser compares what the viewer
//! types against the served value.
//!
//! NOTE: this is a client-side UX gate, not enforced authentication — anyone on
//! the LAN can read `session.json` or the served JS to learn the code.
//! Signalling-layer enforcement is future work.

use rand::seq::SliceRandom;

/// Unambiguous alphabet: no `0 O 1 I L` so the code is easy to read aloud and
/// type from a screen/QR-adjacent label.
const ALPHABET: &[u8] = b"ABCDEFGHJKMNPQRSTUVWXYZ23456789";

/// Characters per group.
const GROUP_LEN: usize = 3;
/// Number of groups.
const GROUPS: usize = 3;

/// Generate a random access code formatted as 3 groups of 3 characters
/// separated by `/`, e.g. `GHF/ABA/6TJ`.
pub fn generate() -> String {
    let mut rng = rand::thread_rng();
    let groups: Vec<String> = (0..GROUPS)
        .map(|_| {
            (0..GROUP_LEN)
                .map(|_| *ALPHABET.choose(&mut rng).expect("alphabet is non-empty") as char)
                .collect::<String>()
        })
        .collect();
    groups.join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_code_has_expected_shape() {
        let code = generate();
        let parts: Vec<&str> = code.split('/').collect();
        assert_eq!(parts.len(), GROUPS, "expected {GROUPS} groups in {code}");
        for p in &parts {
            assert_eq!(p.len(), GROUP_LEN, "group {p} should be {GROUP_LEN} chars");
        }
    }

    #[test]
    fn generated_code_uses_only_unambiguous_alphabet() {
        let code = generate();
        for c in code.chars().filter(|c| *c != '/') {
            assert!(
                ALPHABET.contains(&(c as u8)),
                "char {c} not in the unambiguous alphabet"
            );
        }
        // Explicitly reject the ambiguous characters we excluded.
        for bad in ['0', 'O', '1', 'I', 'L'] {
            assert!(!code.contains(bad), "code {code} contains ambiguous {bad}");
        }
    }

    #[test]
    fn codes_are_not_constant() {
        // Extremely unlikely (1 in ~30^9) to collide across 5 draws if random.
        let codes: std::collections::HashSet<String> = (0..5).map(|_| generate()).collect();
        assert!(codes.len() > 1, "generator produced identical codes");
    }
}
