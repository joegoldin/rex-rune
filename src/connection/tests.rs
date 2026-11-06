use super::*;
use futures_util::{SinkExt, StreamExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{WebSocketStream, tungstenite::Message};

const SESSION: &str = "session:00000000-0000-0000-0000-000000000001";

fn ticket(session: &str, block: &str, expires_in_ms: u64) -> session::DataTicket {
    session::DataTicket {
        session_id: session.into(),
        block_id: block.into(),
        data_ref_id: "terminal".into(),
        subprotocol: Some(wire::PROTOCOL.into()),
        ticket: "single-use".into(),
        expires_in_ms,
    }
}

#[test]
fn tickets_are_scoped_expiring_and_single_use() {
    let mut state = ControlState::default();
    let mut wrong = ticket(SESSION, "wrong", 60_000);
    wrong.subprotocol = Some("other".into());
    state.add_tickets(
        vec![
            ticket(SESSION, "valid", 60_000),
            ticket(SESSION, "expired", 0),
            wrong,
        ],
        Instant::now(),
    );
    assert!(state.take_ticket("other-session", "valid").is_none());
    assert!(state.take_ticket(SESSION, "wrong").is_none());
    assert!(state.take_ticket(SESSION, "expired").is_none());
    assert!(state.take_ticket(SESSION, "valid").unwrap().bundled);
    assert!(state.take_ticket(SESSION, "valid").is_none());
    state.add_tickets(
        vec![ticket(SESSION, "slow", 100)],
        Instant::now() - Duration::from_secs(1),
    );
    assert!(state.take_ticket(SESSION, "slow").is_none());
    state.add_tickets(vec![ticket(SESSION, "removed", 60_000)], Instant::now());
    state.prune(&HostModel::default());
    assert!(state.tickets.is_empty());
}

#[test]
fn selection_and_bootstrap_preserve_explicit_intent() {
    let sessions = vec![session::Session {
        session_id: SESSION.into(),
        label: "work".into(),
    }];
    assert_eq!(select_session(&sessions, None).as_deref(), Some(SESSION));
    assert_eq!(
        select_session(&sessions, Some("work")).as_deref(),
        Some(SESSION)
    );
    assert_eq!(
        select_session(&sessions, Some(SESSION)).as_deref(),
        Some(SESSION)
    );
    assert!(select_session(&sessions, Some("missing")).is_none());
    let mut duplicate = sessions.clone();
    duplicate.push(session::Session {
        session_id: "other".into(),
        label: "work".into(),
    });
    assert!(select_session(&duplicate, Some("work")).is_none());
    for (wanted, creating, expected) in [
        (None, false, None),
        (Some("work"), false, Some(json!([]))),
        (Some("session:bad-id"), false, Some(json!([]))),
        (None, true, Some(json!([]))),
        (Some(SESSION), false, Some(json!([SESSION]))),
    ] {
        let request =
            serde_json::to_value(bootstrap_request(wanted, creating, std::iter::empty())).unwrap();
        assert_eq!(request.get("attach_session_ids").cloned(), expected);
        assert_eq!(request["data_tickets"][0]["subprotocol"], wire::PROTOCOL);
    }
    let retained = bootstrap_request(
        Some(SESSION),
        false,
        [SESSION.to_owned(), SESSION.to_owned()].into_iter(),
    );
    assert_eq!(retained.attach_session_ids, Some(vec![SESSION.into()]));
}

#[allow(clippy::result_large_err)] // tungstenite's callback fixes the error type.
async fn accept(listener: &TcpListener, protocol: &str) -> WebSocketStream<TcpStream> {
    let (tcp, _) = listener.accept().await.unwrap();
    tokio_tungstenite::accept_hdr_async(
        tcp,
        |_: &_, mut response: tokio_tungstenite::tungstenite::handshake::server::Response| {
            response
                .headers_mut()
                .insert("Sec-WebSocket-Protocol", protocol.parse().unwrap());
            Ok(response)
        },
    )
    .await
    .unwrap()
}

async fn recv(ws: &mut WebSocketStream<TcpStream>) -> Value {
    serde_json::from_str(ws.next().await.unwrap().unwrap().to_text().unwrap()).unwrap()
}

async fn send(ws: &mut WebSocketStream<TcpStream>, value: Value) {
    ws.send(Message::Text(value.to_string().into()))
        .await
        .unwrap();
}

async fn welcome(ws: &mut WebSocketStream<TcpStream>, revision: u32) -> Value {
    let hello = recv(ws).await;
    assert_eq!(hello["api_revision"], 8);
    assert_eq!(hello["api_min_revision"], 7);
    send(ws, json!({"type":"welcome", "protocol":"rex.control.v1", "api_revision":revision,
        "limits":{"max_message_bytes":65536}, "client":{"id":"client:test", "reconnect_token":"test-token"}})).await;
    hello
}

fn boot() -> session::BootstrapResponse {
    session::BootstrapResponse {
        client_id: "client:test".into(),
        attach_all: session::AttachAllResponse {
            sessions: vec![session::Session {
                session_id: SESSION.into(),
                label: "snapshot".into(),
            }],
        },
        selected_session: Some(session::SelectedSession {
            session: session::Session {
                session_id: SESSION.into(),
                label: "snapshot".into(),
            },
            view: session::ViewResponse {
                session_id: Some(SESSION.into()),
                label: Some("snapshot".into()),
                revision: 1,
                ..Default::default()
            },
        }),
        ..Default::default()
    }
}

async fn connected(host: &HostConnection) {
    tokio::time::timeout(Duration::from_secs(8), async {
        while !host.model.lock().unwrap().connected {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn bootstrap_is_first_and_newer_pre_response_events_survive() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 8).await;
        let request = recv(&mut ws).await;
        assert_eq!(request["method"], "session.bootstrap");
        // An older destruction must not remove a session present in the snapshot.
        send(&mut ws, json!({"type":"event","seq":9,"payload":{"type":"session_destroyed","payload":{"session_id":SESSION}}})).await;
        // A newer event can arrive BEFORE the bootstrap response.
        send(&mut ws, json!({"type":"event","seq":11,"payload":{"type":"session_event","payload":{
            "session_id":SESSION,"event":{"type":"session_label_changed","payload":{"label":"newer"}}}}})).await;
        send(
            &mut ws,
            json!({"type":"response","id":request["id"],"seq":10,"payload":boot()}),
        )
        .await;
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false);
    connected(&host).await;
    {
        let model = host.model.lock().unwrap();
        assert_eq!(model.sessions[0].label, "newer");
        assert_eq!(model.selected.as_deref(), Some(SESSION));
        assert_eq!(model.protocol.bootstrap_requests, 1);
        assert_eq!(model.protocol.attach_requests, 0);
    }
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn unsupported_negotiated_revision_is_rejected() {
    for revision in [6, 9] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener, "rex.control.v1").await;
            welcome(&mut ws, revision).await;
        });
        assert!(transport::control(&endpoint, None).await.is_err());
        server.await.unwrap();
    }
}

#[test]
fn snapshot_cutoffs_are_scoped_to_the_state_they_replace() {
    let state = ControlState {
        bootstrap_seq: 10,
        list_seq: 15,
        view_seq: HashMap::from([(SESSION.into(), 20)]),
        ..Default::default()
    };
    let event = |seq, name: &str, session: Option<&str>| rexrs::Event {
        seq,
        name: name.into(),
        session_id: session.map(str::to_owned),
        payload: json!({}),
    };
    assert!(!state.includes(&event(20, "block_closed", Some(SESSION))));
    assert!(state.includes(&event(21, "block_closed", Some(SESSION))));
    assert!(state.includes(&event(19, "block_closed", Some("other-session"))));
    assert!(!state.includes(&event(15, "session_destroyed", None)));
    assert!(state.includes(&event(16, "session_destroyed", Some(SESSION))));
    assert!(!state.includes(&event(19, "session_label_changed", Some(SESSION))));
    assert!(state.includes(&event(21, "session_label_changed", Some(SESSION))));
}

#[tokio::test]
async fn failed_bootstrap_closes_and_retries_without_old_identity() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut first = accept(&listener, "rex.control.v1").await;
        welcome(&mut first, 8).await;
        let request = recv(&mut first).await;
        assert_eq!(request["method"], "session.bootstrap");
        send(&mut first, json!({"type":"response","id":request["id"],"error":{"code":"session_not_found","message":"closed during setup","status":404}})).await;
        assert!(matches!(first.next().await, Some(Ok(Message::Close(_)))));
        let mut second = accept(&listener, "rex.control.v1").await;
        let hello = welcome(&mut second, 8).await;
        assert!(hello.get("reconnect_token").is_none());
        let request = recv(&mut second).await;
        assert_eq!(request["method"], "session.bootstrap");
        send(
            &mut second,
            json!({"type":"response","id":request["id"],"seq":1,"payload":boot()}),
        )
        .await;
        assert!(matches!(second.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false);
    connected(&host).await;
    assert_eq!(host.model.lock().unwrap().protocol.bootstrap_requests, 2);
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn rejected_bundle_gets_fresh_ticket_and_new_socket() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        for accepted in [false, true] {
            let mut ws = accept(&listener, wire::PROTOCOL).await;
            let message = ws.next().await.unwrap().unwrap().into_data();
            assert_eq!(message.len(), 17);
            assert_eq!(message[0], 1);
            let mut response = if accepted { vec![0; 32] } else { vec![0x82, 1] };
            if accepted {
                response[0] = 0x81;
                response[24] = 1;
            }
            ws.send(Message::Binary(response.into())).await.unwrap();
        }
    });
    let (sender, commands) = flume::unbounded();
    let host = tokio::spawn(async move {
        for bundled in [true, false] {
            let HostCommand::Ticket {
                session,
                block,
                reply,
            } = commands.recv_async().await.unwrap()
            else {
                panic!("expected ticket");
            };
            assert_eq!(session, SESSION);
            assert_eq!(block, "block");
            assert!(
                reply
                    .send(Ok(TerminalTicket {
                        token: "AAAAAAAAAAAAAAAAAAAAAA".into(),
                        bundled
                    }))
                    .is_ok()
            );
        }
    });
    let (_, welcome) = open_data(&endpoint, SESSION, "block", &sender)
        .await
        .unwrap();
    assert_eq!(welcome.nonce, 1);
    host.await.unwrap();
    server.await.unwrap();
}

const FIXTURE: &str = include_str!("fixtures/block_events.jsonl");
const FIXTURE_BLOCK: &str = "block:01a19b73-2201-7d4a-9b0b-39967ffa0cd5";

#[test]
fn captured_block_events_update_activity_programs_and_notices() {
    use activity::{BlockedKind, ProgramState};
    let mut model = HostModel::default();
    for line in FIXTURE.lines() {
        let payload: Value = serde_json::from_str(line).unwrap();
        let (block, event) = activity::parse_block_event(&payload).expect("a terminal event");
        assert_eq!(block, FIXTURE_BLOCK);
        model.apply_block_event(SESSION, block, event);
    }
    let records = &model.activity[FIXTURE_BLOCK];
    let ids: Vec<_> = records.keys().map(String::as_str).collect();
    // "kid" reported done, then was removed when the foreground process changed.
    assert_eq!(ids, ["", "build", "build/test"]);
    assert_eq!(records[""].state, ProgramState::Working);
    assert_eq!(records[""].app.as_deref(), Some("claude"));
    assert_eq!(records["build"].state, ProgramState::Blocked);
    assert_eq!(records["build"].kind, Some(BlockedKind::Permission));
    assert_eq!(records["build/test"].state, ProgramState::Error);
    // The shell's argv0 is "/usr/bin/bash"; only its basename shows.
    assert_eq!(model.programs[FIXTURE_BLOCK], "bash");
    let notices: Vec<_> = model
        .notices
        .iter()
        .map(|n| (n.session.as_str(), n.title.as_str(), n.body.as_str()))
        .collect();
    assert_eq!(
        notices,
        [
            (SESSION, "", "hello there"),
            (SESSION, "Title", "Body text")
        ]
    );
}

#[test]
fn a_title_lasts_while_the_program_that_set_it_runs() {
    let mut model = HostModel::default();
    let replay = |model: &mut HostModel, lines: &str| {
        for line in lines.lines() {
            let payload: Value = serde_json::from_str(line).unwrap();
            let (block, event) = activity::parse_block_event(&payload).unwrap();
            model.apply_block_event(SESSION, block, event);
        }
    };
    let block = "block:01a19e0b-91d8-7213-8fda-04f8b42016e6";
    let fixture: Vec<_> = include_str!("fixtures/title_events.jsonl")
        .lines()
        .collect();
    // A program titles the terminal just before the server reports it in the foreground.
    replay(&mut model, fixture[0]);
    replay(&mut model, fixture[1]);
    assert_eq!(model.title(block), Some("OC | USB device exploration"));
    // Back at the prompt, the shell titles it again.
    replay(&mut model, &fixture[2..].join("\n"));
    assert_eq!(model.title(block), Some("shuv@shuvdev:~"));

    // A program that exits without resetting the title leaves a stale one behind.
    let title =
        activity::parse_title(&json!({"title":"nvim notes.md","owner":{"pid":7,"name":"nvim"}}));
    model.apply_block_event(SESSION, "b".into(), title);
    let foreground = |pids: &[u64]| {
        activity::parse_process(&json!({
            "foreground": {"pid": pids[0], "name": "x"},
            "ancestors": pids[1..].iter().map(|p| json!({"pid": p})).collect::<Vec<_>>(),
        }))
    };
    model.apply_block_event(SESSION, "b".into(), foreground(&[7, 1]));
    assert_eq!(model.title("b"), Some("nvim notes.md"));
    // A child of the titling program keeps its title; the shell coming back drops it.
    model.apply_block_event(SESSION, "b".into(), foreground(&[9, 7, 1]));
    assert_eq!(model.title("b"), Some("nvim notes.md"));
    model.apply_block_event(SESSION, "b".into(), foreground(&[1]));
    assert_eq!(model.title("b"), None);
    // A cleared title is gone; an unknown owner can't go stale.
    let clear = activity::parse_title(&json!({"title":""}));
    model.apply_block_event(SESSION, "b".into(), clear);
    assert!(!model.titles.contains_key("b"));
    model.apply_block_event(
        SESSION,
        "b".into(),
        activity::parse_title(&json!({"title":"t"})),
    );
    assert_eq!(model.title("b"), Some("t"));
}

#[test]
fn unknown_states_kinds_and_events_stay_quiet() {
    use activity::{BlockEvent, ProgramState, StatusRecord};
    let record = StatusRecord::parse(&json!({"state":"sleeping","kind":"coffee","app":"x"}));
    assert_eq!(record.state, ProgramState::Idle);
    assert_eq!(record.kind, None);
    assert_eq!(record.app.as_deref(), Some("x"));
    let event = |creator: &str, name: &str| json!({"block_id":"b","creator_name":creator,"name":name,"payload":{}});
    assert!(activity::parse_block_event(&event("other.creator", "process_changed")).is_none());
    assert!(activity::parse_block_event(&event(activity::TERMINAL, "bell")).is_none());
    assert_eq!(
        activity::parse_block_event(&event(activity::TERMINAL, "process_changed")),
        Some((
            "b".into(),
            BlockEvent::Process {
                name: None,
                pids: Vec::new()
            }
        ))
    );
    assert!(ProgramState::Blocked > ProgramState::Error);
    assert!(ProgramState::Error > ProgramState::Working);
    assert!(ProgramState::Working > ProgramState::Done);
    assert!(ProgramState::Done > ProgramState::Idle);
}

#[test]
fn program_names_prefer_the_script_and_drop_login_dashes() {
    let name = |v: Value| activity::program_name(&v);
    assert_eq!(
        name(json!({"name":"node","script":"/home/u/.bun/bin/pi","argv0":"node"})).as_deref(),
        Some("pi")
    );
    assert_eq!(
        name(json!({"name":"zsh","argv0":"-zsh"})).as_deref(),
        Some("zsh")
    );
    assert_eq!(name(json!({"name":"nvim"})).as_deref(), Some("nvim"));
    assert_eq!(name(Value::Null), None);
}

const OTHER: &str = "session:00000000-0000-0000-0000-000000000002";

async fn reply(ws: &mut WebSocketStream<TcpStream>, request: &Value, seq: u64, payload: Value) {
    send(
        ws,
        json!({"type":"response","id":request["id"],"seq":seq,"payload":payload}),
    )
    .await;
}

#[tokio::test]
async fn activity_follows_every_session_and_queries_the_selected_one() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 8).await;
        let request = recv(&mut ws).await;
        assert_eq!(request["method"], "session.bootstrap");
        assert_eq!(request["payload"]["event_scope"], "all");
        let mut boot = serde_json::to_value(boot()).unwrap();
        boot["attach_all"]["sessions"]
            .as_array_mut()
            .unwrap()
            .push(json!({"session_id":OTHER,"label":"other"}));
        boot["selected_session"]["view"]["windows"] = json!([{"window_id":"w",
            "layers":[{"kind":"tiled","layout":{"block_id":"block:a"},
                "blocks":[{"block_id":"block:a","creator_name":activity::TERMINAL}]}]}]);
        reply(&mut ws, &request, 1, boot).await;
        // Hidden hosts stream nothing, so both questions are asked.
        let status = recv(&mut ws).await;
        assert_eq!(status["method"], "com.superlogical.terminal.program_status");
        assert_eq!(status["payload"]["block_id"], "block:a");
        reply(
            &mut ws,
            &status,
            2,
            json!({"records":[{"state":"blocked","kind":"question","app":"codex"}]}),
        )
        .await;
        let title = recv(&mut ws).await;
        assert_eq!(title["method"], "com.superlogical.terminal.title");
        reply(
            &mut ws,
            &title,
            3,
            json!({"title":"Codex · fix tests","owner":{"pid":41,"name":"node"}}),
        )
        .await;
        let process = recv(&mut ws).await;
        assert_eq!(process["method"], "com.superlogical.terminal.process");
        reply(
            &mut ws,
            &process,
            4,
            json!({"foreground":{"name":"node","argv0":"codex"}}),
        )
        .await;
        // attach_all replaces the scope, so after session_created it must still ask for all.
        send(&mut ws, json!({"type":"event","seq":5,"payload":{"type":"session_created","payload":{"session_id":OTHER}}})).await;
        let list = recv(&mut ws).await;
        assert_eq!(list["method"], "session.attach_all");
        assert_eq!(list["payload"]["event_scope"], "all");
        reply(
            &mut ws,
            &list,
            6,
            json!({"sessions":[{"session_id":SESSION,"label":"snapshot"},
            {"session_id":OTHER,"label":"other"}]}),
        )
        .await;
        // A report from an unattached session: its view is fetched once to place it.
        let report = |seq: u64| {
            json!({"type":"event","seq":seq,"payload":{"type":"session_event","payload":{
            "session_id":OTHER,"event":{"type":"block_event","payload":{"block_id":"block:z",
                "creator_name":activity::TERMINAL,"name":"program_status_changed",
                "payload":{"id":"","record":{"state":"working"}}}}}}})
        };
        send(&mut ws, report(7)).await;
        let view = recv(&mut ws).await;
        assert_eq!(view["method"], "session.view");
        assert_eq!(view["payload"]["session_id"], OTHER);
        send(&mut ws, json!({"type":"response","id":view["id"],"error":{"code":"conflict","message":"no","status":409}})).await;
        send(&mut ws, report(8)).await;
        // No second session.view: the next message is the client closing.
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false);
    tokio::time::timeout(Duration::from_secs(8), async {
        loop {
            {
                let m = host.model.lock().unwrap();
                if m.activity.contains_key("block:z") && m.programs.contains_key("block:a") {
                    break;
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    {
        let m = host.model.lock().unwrap();
        let a = &m.activity["block:a"][""];
        assert_eq!(a.state, activity::ProgramState::Blocked);
        assert_eq!(m.title("block:a"), Some("Codex · fix tests"));
        assert_eq!(a.app.as_deref(), Some("codex"));
        assert_eq!(m.programs["block:a"], "codex");
        assert_eq!(m.sessions.len(), 2);
    }
    // Let the second report land before closing.
    tokio::time::sleep(Duration::from_millis(100)).await;
    drop(host);
    server.await.unwrap();
}

#[tokio::test]
async fn destroying_a_session_forgets_it_without_an_error() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let endpoint = format!("http://{}", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        let mut ws = accept(&listener, "rex.control.v1").await;
        welcome(&mut ws, 8).await;
        let request = recv(&mut ws).await;
        reply(&mut ws, &request, 1, serde_json::to_value(boot()).unwrap()).await;
        let destroy = recv(&mut ws).await;
        assert_eq!(destroy["method"], "session.destroy");
        reply(&mut ws, &destroy, 2, json!({})).await;
        // The client re-reads the session it acted on, which is gone now.
        let view = recv(&mut ws).await;
        assert_eq!(view["method"], "session.view");
        send(
            &mut ws,
            json!({"type":"response","id":view["id"],"error":{"code":"session_not_found",
            "message":"session not found","status":404}}),
        )
        .await;
        assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
    });
    let host = HostConnection::spawn(endpoint, None, None, None, false);
    connected(&host).await;
    host.commands
        .send(HostCommand::Request {
            method: "session.destroy".into(),
            payload: json!({"session_id": SESSION}),
        })
        .unwrap();
    tokio::time::timeout(Duration::from_secs(8), async {
        while !host.model.lock().unwrap().sessions.is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    {
        let m = host.model.lock().unwrap();
        assert_eq!(m.notice, None);
        assert_eq!(m.selected, None);
        assert!(m.connected);
    }
    drop(host);
    server.await.unwrap();
}

#[test]
fn clipboard_events_are_not_replaced_by_a_view_snapshot() {
    let state = ControlState {
        bootstrap_seq: 10,
        view_seq: HashMap::from([(SESSION.into(), 20)]),
        ..Default::default()
    };
    for name in ["clipboard_written", "selection_copied"] {
        let event = rexrs::Event {
            seq: 19,
            name: "block_event".into(),
            session_id: Some(SESSION.into()),
            payload: json!({"creator_name": activity::TERMINAL, "name": name,
                "block_id": "block:copy", "payload": {"id": 1, "bytes": 4, "text": "copy"}}),
        };
        assert!(
            state.includes(&event),
            "a view contains no clipboard text: {name}"
        );
        assert!(!state.includes(&rexrs::Event { seq: 9, ..event }));
    }
}

#[tokio::test]
async fn clipboard_reads_pin_ids_and_count_utf8_bytes_for_both_copy_events() {
    for (event, method) in [
        (
            "clipboard_written",
            "com.superlogical.terminal.clipboard_read",
        ),
        ("selection_copied", "com.superlogical.terminal.blob_read"),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener, "rex.control.v1").await;
            welcome(&mut ws, 8).await;
            let mut offset = 0;
            for (index, data) in ["世", "界🦀\n"].into_iter().enumerate() {
                let request = recv(&mut ws).await;
                assert_eq!(request["method"], method);
                assert_eq!(request["payload"]["session_id"], SESSION);
                assert_eq!(request["payload"]["block_id"], "block:copy");
                assert_eq!(
                    request["payload"]["args"],
                    json!({"id": 42, "offset": offset, "max_bytes": 49152})
                );
                offset += data.len();
                send(
                    &mut ws,
                    json!({"type":"response", "id":request["id"], "payload":{
                    "id":42,"data":data,"total":11,"eof": index == 1}}),
                )
                .await;
            }
            assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
        });
        let mut client = transport::control(&endpoint, None).await.unwrap();
        let payload =
            json!({"name": event, "block_id": "block:copy", "payload": {"id": 42, "bytes": 11}});
        let copy = clipboard::read(&mut client, SESSION, &payload)
            .await
            .unwrap();
        assert_eq!(copy.text, "世界🦀\n");
        assert_eq!(copy.session, SESSION);
        assert_eq!(copy.block, "block:copy");
        client.close().await.unwrap();
        server.await.unwrap();
    }
}

#[tokio::test]
async fn clipboard_reads_reject_stalled_truncated_changed_and_oversized_results() {
    for part in [
        json!({"id":42,"data":"","total":5,"eof":false}),
        json!({"id":42,"data":"copy","total":5,"eof":true}),
        json!({"id":43,"data":"copy","total":4,"eof":true}),
        json!({"id":42,"data":"copy","total":3,"eof":true}),
        json!({"id":42,"data":"","total":-1,"eof":true}),
        json!({"id":42,"data":"","total":8 * 1024 * 1024 + 1,"eof":false}),
    ] {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let mut ws = accept(&listener, "rex.control.v1").await;
            welcome(&mut ws, 8).await;
            let request = recv(&mut ws).await;
            send(
                &mut ws,
                json!({"type":"response","id":request["id"],"payload":part}),
            )
            .await;
            assert!(matches!(ws.next().await, Some(Ok(Message::Close(_)))));
        });
        let mut client = transport::control(&endpoint, None).await.unwrap();
        assert!(
            clipboard::read(
                &mut client,
                SESSION,
                &json!({"name":"clipboard_written",
            "block_id":"block:copy","payload":{"id":42}})
            )
            .await
            .is_err()
        );
        client.close().await.unwrap();
        server.await.unwrap();
    }
}
