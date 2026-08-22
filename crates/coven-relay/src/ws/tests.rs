use super::*;
use axum::http::HeaderValue;

fn canonical(seed: char) -> String {
    let mut value = seed.to_string().repeat(42);
    value.push('A');
    value
}

fn headers(credential: &str) -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {credential}")).unwrap(),
    );
    headers
}

#[test]
fn request_parser_requires_canonical_version_room_role_and_credential() {
    let room = canonical('A');
    let credential = canonical('C');
    let query = format!("role=host&room={room}&v=1");
    let parsed = RelayRequest::parse(Some(&query), &headers(&credential)).unwrap();
    assert_eq!(parsed.room_id, room);
    assert_eq!(parsed.credential, credential);
    assert_eq!(parsed.role, PeerRole::Host);

    for query in [
        format!("room={room}&role=host"),
        format!("v=2&room={room}&role=host"),
        format!("v=1&room={room}&room={room}&role=host"),
        format!("v=1&room={room}&role=admin"),
        "v=1&room=not-base64url&role=host".to_owned(),
        format!("v=1&room={room}%3d&role=host"),
        format!("v=1&room={room}&role=host&extra=value"),
    ] {
        assert!(RelayRequest::parse(Some(&query), &headers(&credential)).is_err());
    }
    assert!(RelayRequest::parse(Some(&query), &HeaderMap::new()).is_err());
}

#[test]
fn authorization_header_must_be_unique_and_canonical() {
    let credential = canonical('D');
    let mut duplicate = headers(&credential);
    duplicate.append(
        AUTHORIZATION,
        HeaderValue::from_str(&format!("Bearer {credential}")).unwrap(),
    );
    assert_eq!(
        bearer_credential(&duplicate).unwrap_err(),
        RequestError::AuthorizationRequired
    );

    let mut malformed = HeaderMap::new();
    malformed.insert(AUTHORIZATION, HeaderValue::from_static("Basic value"));
    assert_eq!(
        bearer_credential(&malformed).unwrap_err(),
        RequestError::AuthorizationRequired
    );
}

#[test]
fn base64url_validator_rejects_noncanonical_tail_bits() {
    let canonical = canonical('E');
    assert!(is_canonical_32_byte_base64url(&canonical));
    let mut noncanonical = canonical;
    noncanonical.pop();
    noncanonical.push('B');
    assert!(!is_canonical_32_byte_base64url(&noncanonical));
    assert!(!is_canonical_32_byte_base64url("short"));
}

#[test]
fn secret_comparison_checks_length_and_every_byte() {
    let credential = canonical('F');
    let other = canonical('G');
    assert!(secret_eq(credential.as_bytes(), credential.as_bytes()));
    assert!(!secret_eq(credential.as_bytes(), other.as_bytes()));
    assert!(!secret_eq(credential.as_bytes(), b"short"));
}

#[tokio::test]
async fn room_accepts_one_peer_per_role_with_the_same_credential() {
    let state = RelayState::with_limits(RelayLimits {
        max_rooms: 2,
        channel_capacity: 2,
    });
    let room = canonical('H');
    let credential = canonical('I');
    let other_credential = canonical('J');

    let host = state
        .register(&room, &credential, PeerRole::Host)
        .await
        .unwrap();
    assert_eq!(
        state
            .register(&room, &credential, PeerRole::Host)
            .await
            .err(),
        Some(RegistrationError::RoleOccupied)
    );
    assert_eq!(
        state
            .register(&room, &other_credential, PeerRole::Client)
            .await
            .err(),
        Some(RegistrationError::AuthorizationFailed)
    );
    let client = state
        .register(&room, &credential, PeerRole::Client)
        .await
        .unwrap();
    assert!(state.peer_sender(&room, PeerRole::Host).await.is_some());
    assert!(state.peer_sender(&room, PeerRole::Client).await.is_some());

    state
        .unregister(&room, PeerRole::Client, client.peer_id)
        .await;
    state
        .unregister(&room, PeerRole::Host, host.peer_id)
        .await;
    assert_eq!(state.room_count().await, 0);
}

#[tokio::test]
async fn room_capacity_is_bounded_and_released_after_disconnect() {
    let state = RelayState::with_limits(RelayLimits {
        max_rooms: 1,
        channel_capacity: 1,
    });
    let first_room = canonical('K');
    let second_room = canonical('L');
    let credential = canonical('M');

    let first = state
        .register(&first_room, &credential, PeerRole::Host)
        .await
        .unwrap();
    assert_eq!(
        state
            .register(&second_room, &credential, PeerRole::Host)
            .await
            .err(),
        Some(RegistrationError::RelayFull)
    );
    state
        .unregister(&first_room, PeerRole::Host, first.peer_id)
        .await;
    let replacement = state
        .register(&second_room, &credential, PeerRole::Host)
        .await
        .unwrap();
    state
        .unregister(&second_room, PeerRole::Host, replacement.peer_id)
        .await;
    assert_eq!(state.room_count().await, 0);
}

#[tokio::test]
async fn binary_frames_forward_and_backpressure_fails_closed() {
    let state = RelayState::with_limits(RelayLimits {
        max_rooms: 1,
        channel_capacity: 1,
    });
    let room = canonical('N');
    let credential = canonical('O');
    let host = state
        .register(&room, &credential, PeerRole::Host)
        .await
        .unwrap();
    let mut client = state
        .register(&room, &credential, PeerRole::Client)
        .await
        .unwrap();

    forward_to_peer(
        &state,
        &room,
        PeerRole::Host,
        Message::binary(b"ciphertext".to_vec()),
    )
    .await
    .unwrap();
    assert_eq!(
        client.inbox.recv().await,
        Some(Message::binary(b"ciphertext".to_vec()))
    );

    forward_to_peer(
        &state,
        &room,
        PeerRole::Host,
        Message::binary(b"first".to_vec()),
    )
    .await
    .unwrap();
    assert!(forward_to_peer(
        &state,
        &room,
        PeerRole::Host,
        Message::binary(b"second".to_vec()),
    )
    .await
    .is_err());

    state
        .unregister(&room, PeerRole::Client, client.peer_id)
        .await;
    state
        .unregister(&room, PeerRole::Host, host.peer_id)
        .await;
}

#[tokio::test]
async fn disconnect_notifies_the_remaining_peer() {
    let state = RelayState::with_limits(RelayLimits {
        max_rooms: 1,
        channel_capacity: 2,
    });
    let room = canonical('P');
    let credential = canonical('Q');
    let mut host = state
        .register(&room, &credential, PeerRole::Host)
        .await
        .unwrap();
    let client = state
        .register(&room, &credential, PeerRole::Client)
        .await
        .unwrap();

    state
        .unregister(&room, PeerRole::Client, client.peer_id)
        .await;
    assert!(matches!(host.inbox.recv().await, Some(Message::Close(_))));
    state
        .unregister(&room, PeerRole::Host, host.peer_id)
        .await;
}
