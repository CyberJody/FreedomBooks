use super::*;

#[test]
fn generated_sign_in_secrets_are_independent_and_rfc7636_length() {
    let state = random_secret().unwrap();
    let nonce = random_secret().unwrap();
    let pkce = PkcePair::generate().unwrap();
    assert_eq!(state.expose_secret().len(), 43);
    assert_eq!(nonce.expose_secret().len(), 43);
    assert_eq!(pkce.verifier.expose_secret().len(), 43);
    assert_ne!(state.expose_secret(), nonce.expose_secret());
    assert_ne!(nonce.expose_secret(), pkce.verifier.expose_secret());
    verify_pkce(pkce.verifier.expose_secret(), &pkce.challenge).unwrap();
}

#[test]
fn comparison_refuses_same_length_different_length_and_empty_mismatches() {
    assert!(equal(b"expected", b"expected"));
    assert!(!equal(b"expected", b"Expected"));
    assert!(!equal(b"expected", b"expected-longer"));
    assert!(!equal(b"expected", b""));
}

#[test]
fn redirect_path_is_raw_exact_not_a_normalized_url() {
    assert!(valid_path("/auth/openai/callback"));
    assert!(valid_path("/oauth-callback_1"));
    for path in [
        "//host/callback",
        "/foo/../callback",
        "/%63allback",
        "/callback?code=1",
        "/callback#fragment",
        "/callback\\",
        "/callback\r\nInjected: header",
    ] {
        assert!(!valid_path(path));
    }
}

#[test]
fn oidc_nonce_is_mandatory_and_never_exposed_by_grant_debug() {
    let grant = OAuthGrant {
        code: SecretString::from("CODE_SECRET_SENTINEL"),
        verifier: SecretString::from("VERIFIER_SECRET_SENTINEL"),
        nonce: SecretString::from("NONCE_SECRET_SENTINEL"),
        redirect_uri: "http://127.0.0.1:1234/callback".into(),
        token_endpoint: SecretString::from("https://provider.example/token"),
        client_id: "public-client".into(),
        returned_client_id: None,
    };
    grant.verify_nonce("NONCE_SECRET_SENTINEL").unwrap();
    assert_eq!(grant.verify_nonce(""), Err(PluginError::OAuthNonce));
    assert_eq!(
        grant.verify_nonce("NONCE_SECRET_SENTINE!"),
        Err(PluginError::OAuthNonce)
    );
    for sentinel in [
        "CODE_SECRET_SENTINEL",
        "VERIFIER_SECRET_SENTINEL",
        "NONCE_SECRET_SENTINEL",
    ] {
        assert!(!format!("{grant:?}").contains(sentinel));
    }
}

fn reauthorization_config() -> OAuthConfig {
    let mut config = OAuthConfig::new(
        OAuthEndpoints::new(
            "https://provider.example/authorize",
            "https://provider.example/token",
        )
        .unwrap(),
        "original-client".into(),
        vec!["openid".into(), "email".into()],
        "/callback".into(),
    );
    config.extra_authorization_parameters = vec![
        ("id_token_hint".into(), "synthetic-hint.+/=&% token".into()),
        ("resource".into(), "https://resource.example/".into()),
        ("prompt".into(), "none".into()),
    ];
    config
}

#[test]
fn pending_config_discards_extra_parameters_but_preserves_reauthorization_url() {
    let config = reauthorization_config();
    let expected_extras = config.extra_authorization_parameters.clone();
    let flow = OAuthLoopback::start(config).unwrap();
    assert!(flow.config.extra_authorization_parameters.is_empty());
    assert!(flow.config.endpoints.authorization.query().is_none());

    let url = Url::parse(flow.authorization_url().expose_secret()).unwrap();
    let mut expected = vec![
        ("response_type".into(), "code".into()),
        ("client_id".into(), "original-client".into()),
        ("redirect_uri".into(), flow.redirect_uri().into()),
        ("scope".into(), "openid email".into()),
        ("state".into(), flow.state.expose_secret().into()),
        ("nonce".into(), flow.nonce.expose_secret().into()),
        ("code_challenge".into(), flow.pkce.challenge.clone()),
        ("code_challenge_method".into(), "S256".into()),
    ];
    expected.extend(expected_extras);
    assert_eq!(url.query_pairs().into_owned().collect::<Vec<_>>(), expected);
    assert_eq!(flow.config.client_id, "original-client");
    assert_eq!(
        flow.config.endpoints.token.as_str(),
        "https://provider.example/token"
    );
    assert!(!format!("{flow:?}").contains("synthetic-hint"));
}

#[test]
fn extra_parameters_do_not_bypass_early_or_parameter_validation() {
    for invalid in 0..5 {
        let mut config = reauthorization_config();
        match invalid {
            0 => config.client_id.clear(),
            1 => config.timeout = Duration::ZERO,
            2 => config.redirect_path = "//invalid".into(),
            3 => config
                .extra_authorization_parameters
                .push(("state".into(), "override".into())),
            4 => config
                .extra_authorization_parameters
                .push(("id_token_hint".into(), "duplicate".into())),
            _ => unreachable!(),
        }
        assert!(matches!(
            OAuthLoopback::start(config),
            Err(PluginError::InvalidConfiguration)
        ));
    }
}

#[test]
fn stray_requests_do_not_consume_the_real_callback() {
    let mut config = reauthorization_config();
    config.timeout = Duration::from_secs(3);
    let flow = OAuthLoopback::start(config).unwrap();
    let address = flow.local_addr().unwrap();
    let state = flow.state.expose_secret().to_string();
    let worker = thread::spawn(move || flow.wait());
    for target in ["/favicon.ico", "/wrong-path?state=stray", "/callback", "/callback?code=stray"] {
        let mut stream = TcpStream::connect(address).unwrap();
        write!(stream, "GET {target} HTTP/1.1\r\nHost: {address}\r\n\r\n").unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).unwrap();
        assert!(response.starts_with("HTTP/1.1 400"));
        assert!(!worker.is_finished());
    }
    let mut stream = TcpStream::connect(address).unwrap();
    write!(stream, "GET /callback?state={state}&code=synthetic-code HTTP/1.1\r\nHost: {address}\r\n\r\n").unwrap();
    let mut response = String::new();
    stream.read_to_string(&mut response).unwrap();
    assert!(response.starts_with("HTTP/1.1 200"));
    assert!(worker.join().unwrap().unwrap().code.expose_secret() == "synthetic-code");
}
