use super::{TARGET_SPECIFICATION, override_target_specification};

fn features(specification: &str) -> Vec<&str> {
    let line = specification
        .lines()
        .find(|line| line.contains(r#""features""#))
        .expect("Specification has features; qed");
    let list = line
        .split('"')
        .nth(3)
        .expect("Features are a quoted string; qed");
    list.split(',').collect()
}

#[test]
fn no_overrides_is_the_checked_in_specification() {
    let specification = override_target_specification(None, None).unwrap();
    assert_eq!(specification, TARGET_SPECIFICATION);
}

#[test]
fn empty_features_change_nothing() {
    let specification = override_target_specification(None, Some(String::new())).unwrap();
    assert_eq!(specification, TARGET_SPECIFICATION);
}

#[test]
fn subtracting_a_feature_the_specification_enables_replaces_it() {
    assert!(features(TARGET_SPECIFICATION).contains(&"+m"));

    let specification =
        override_target_specification(None, Some("-m,+interpreter-target".to_string())).unwrap();
    let features = features(&specification);

    assert!(!features.contains(&"+m"));
    assert_eq!(
        features.iter().filter(|feature| **feature == "-m").count(),
        1
    );
    assert_eq!(features.last(), Some(&"+interpreter-target"));
}

#[test]
fn a_later_override_wins_over_an_earlier_one() {
    let specification = override_target_specification(None, Some("-m,+m".to_string())).unwrap();
    let features = features(&specification);

    assert!(!features.contains(&"-m"));
    assert_eq!(
        features.iter().filter(|feature| **feature == "+m").count(),
        1
    );
}

#[test]
fn features_without_a_sign_are_rejected() {
    override_target_specification(None, Some("m".to_string())).unwrap_err();
}

#[test]
fn cpu_is_replaced() {
    let specification =
        override_target_specification(Some("generic-interpreter-rv64".to_string()), None).unwrap();
    assert!(specification.contains(r#""cpu": "generic-interpreter-rv64","#));
    assert!(!specification.contains(r#""cpu": "generic-rv64","#));
}
