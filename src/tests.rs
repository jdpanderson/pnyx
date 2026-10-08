use super::*;

fn config(protected: bool, acceptors: usize) -> Config<usize> {
    Config {
        number: 0,
        protected,
        acceptors: (0..acceptors).collect(),
    }
}

#[test]
fn quorums() {
    let relaxed = [1, 1, 2, 2, 3, 3, 4, 4];
    let protected = [1, 2, 2, 3, 3, 4, 4, 5];
    for n in 0..8 {
        assert_eq!(quorum(n, false), relaxed[n], "relaxed, {n} acceptors");
        assert_eq!(quorum(n, true), protected[n], "protected, {n} acceptors");
    }
}

#[test]
fn only_configurations_with_no_acceptors_or_one_protected_cannot_agree() {
    assert!(!config(false, 0).can_agree());
    assert!(!config(true, 0).can_agree());
    assert!(!config(true, 1).can_agree());
    assert!(config(false, 1).can_agree());
    for n in 2..8 {
        assert!(config(false, n).can_agree(), "relaxed, {n} acceptors");
        assert!(config(true, n).can_agree(), "protected, {n} acceptors");
    }
}
