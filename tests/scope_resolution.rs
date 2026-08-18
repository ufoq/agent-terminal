use agent_terminal::paths::{PI_SESSION_ID_ENV, SCOPE_ENV, resolve_scope_from};

/// Builds a lookup closure from a static map of env-var → value.
/// Keys absent from the map are treated as unset; `None` values are treated
/// as unset (so the caller can explicitly express "not present").
fn env_lookup<'a>(envs: &'a [(&'a str, Option<&'a str>)]) -> impl Fn(&str) -> Option<String> + 'a {
    move |var: &str| {
        envs.iter()
            .find(|(k, _)| *k == var)
            .and_then(|(_, v)| v.map(str::to_owned))
    }
}

#[test]
fn explicit_scope_wins_over_pi_session() {
    let env = [
        (SCOPE_ENV, Some("explicit")),
        (PI_SESSION_ID_ENV, Some("pi-session")),
    ];
    assert_eq!(resolve_scope_from(env_lookup(&env)), "explicit");
}

#[test]
fn pi_session_used_when_scope_unset() {
    let env = [(SCOPE_ENV, None), (PI_SESSION_ID_ENV, Some("pi-session"))];
    assert_eq!(resolve_scope_from(env_lookup(&env)), "pi-session");
}

#[test]
fn standalone_when_both_unset() {
    let env: [(&str, Option<&str>); 2] = [(SCOPE_ENV, None), (PI_SESSION_ID_ENV, None)];
    assert_eq!(resolve_scope_from(env_lookup(&env)), "standalone");
}

#[test]
fn empty_scope_falls_through_to_pi_session() {
    let env = [
        (SCOPE_ENV, Some("")),
        (PI_SESSION_ID_ENV, Some("pi-session")),
    ];
    assert_eq!(resolve_scope_from(env_lookup(&env)), "pi-session");
}

#[test]
fn empty_pi_session_falls_to_standalone() {
    let env: [(&str, Option<&str>); 2] = [(SCOPE_ENV, None), (PI_SESSION_ID_ENV, Some(""))];
    assert_eq!(resolve_scope_from(env_lookup(&env)), "standalone");
}

#[test]
fn whitespace_scope_treated_as_absent() {
    let env: [(&str, Option<&str>); 2] = [(SCOPE_ENV, Some("   ")), (PI_SESSION_ID_ENV, None)];
    assert_eq!(resolve_scope_from(env_lookup(&env)), "standalone");
}

#[test]
fn whitespace_pi_session_treated_as_absent() {
    let env: [(&str, Option<&str>); 2] = [(SCOPE_ENV, None), (PI_SESSION_ID_ENV, Some("  "))];
    assert_eq!(resolve_scope_from(env_lookup(&env)), "standalone");
}

#[test]
fn scope_absent_completely_treated_as_unset() {
    // No entries at all — both vars absent from the lookup
    let env: [(&str, Option<&str>); 0] = [];
    assert_eq!(resolve_scope_from(env_lookup(&env)), "standalone");
}
