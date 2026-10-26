use super::{fake::FakeLab, *};
use std::time::Duration;

fn request() -> LabRequest {
    LabRequest {
        passes: 3,
        cold: false,
        follow_up: None,
        server: None,
    }
}

async fn next(events: &async_channel::Receiver<LabEvent>) -> LabEvent {
    tokio::time::timeout(Duration::from_secs(5), events.recv())
        .await
        .expect("event in time")
        .expect("channel open")
}

#[tokio::test]
async fn check_and_list_answer_with_their_own_ids() {
    let fake = Arc::new(FakeLab::default());
    let (commands, events) = start(&tokio::runtime::Handle::current(), fake.clone());
    commands
        .send(LabCommand::Check {
            id: 7,
            request: request(),
        })
        .await
        .unwrap();
    assert!(matches!(
        next(&events).await,
        LabEvent::Checked { id: 7, result: Ok(checked) } if checked.description == "Runs 3 passes." && checked.server == "fake"
    ));
    *fake.check_error.lock().unwrap() = Some("No server.".into());
    commands
        .send(LabCommand::Check {
            id: 8,
            request: request(),
        })
        .await
        .unwrap();
    assert!(matches!(
        next(&events).await,
        LabEvent::Checked { id: 8, result: Err(e) } if e == "No server."
    ));
    commands.send(LabCommand::List { id: 9 }).await.unwrap();
    assert!(matches!(
        next(&events).await,
        LabEvent::Listed { id: 9, .. }
    ));
}

#[tokio::test]
async fn a_second_run_is_refused_while_one_is_in_flight_and_cancel_ends_the_first() {
    let fake = Arc::new(FakeLab {
        hang: true,
        ..FakeLab::default()
    });
    let (commands, events) = start(&tokio::runtime::Handle::current(), fake.clone());
    let cancel = Arc::new(AtomicBool::new(false));
    commands
        .send(LabCommand::Run {
            id: 1,
            request: request(),
            cancel: cancel.clone(),
        })
        .await
        .unwrap();
    assert!(matches!(
        next(&events).await,
        LabEvent::Progress { id: 1, progress } if progress.done == 1 && progress.of == 3
    ));
    commands
        .send(LabCommand::Run {
            id: 2,
            request: request(),
            cancel: Arc::new(AtomicBool::new(false)),
        })
        .await
        .unwrap();
    assert!(matches!(
        next(&events).await,
        LabEvent::Finished { id: 2, result: Err(e) } if e.contains("already in progress")
    ));
    assert_eq!(
        fake.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|c| c.starts_with("run"))
            .count(),
        1,
        "the refused run never reached the backend"
    );
    cancel.store(true, Ordering::Relaxed);
    assert!(matches!(
        next(&events).await,
        LabEvent::Finished {
            id: 1,
            result: Err(_)
        }
    ));
    // The slot is free again.
    commands
        .send(LabCommand::Run {
            id: 3,
            request: request(),
            cancel: Arc::new(AtomicBool::new(true)),
        })
        .await
        .unwrap();
    loop {
        if let LabEvent::Finished { id, result } = next(&events).await {
            assert_eq!(id, 3);
            assert!(result.unwrap_err().contains("cancelled"));
            break;
        }
    }
}

#[tokio::test]
async fn comparing_a_missing_experiment_says_so() {
    let fake = Arc::new(FakeLab::default());
    let (commands, events) = start(&tokio::runtime::Handle::current(), fake);
    commands
        .send(LabCommand::Compare { id: 4, a: 1, b: 2 })
        .await
        .unwrap();
    assert!(matches!(
        next(&events).await,
        LabEvent::Compared { id: 4, result: Err(e) } if e.contains("no longer exists")
    ));
}

#[test]
fn model_facts_hash_the_file_and_refuse_a_directory() {
    let dir = std::env::temp_dir().join(format!("kvlm-lab-facts-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join("m.gguf");
    std::fs::write(&file, b"abc").unwrap();
    let (sha, layers) = model_facts(&file).unwrap();
    assert_eq!(
        sha,
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(layers, None, "not a GGUF file");
    assert_eq!(model_facts(&file).unwrap().0, sha, "cached read agrees");
    assert!(model_facts(&dir).is_err());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn memory_topology_stays_unknown_without_a_report() {
    let mut hardware = HardwareProfile {
        profile_hash: None,
        cpu: None,
        system_ram: None,
        gpus: vec![],
        issues: vec![],
        captured_at_unix_ms: 1,
    };
    assert_eq!(unified_memory(&hardware), None);
    let _ = &mut hardware;
}

#[tokio::test]
async fn a_panicking_run_ends_with_an_error_and_frees_the_slot() {
    let fake = Arc::new(FakeLab {
        panic: true,
        ..FakeLab::default()
    });
    let (commands, events) = start(&tokio::runtime::Handle::current(), fake);
    for id in [1, 2] {
        commands
            .send(LabCommand::Run {
                id,
                request: request(),
                cancel: Arc::new(AtomicBool::new(false)),
            })
            .await
            .unwrap();
        loop {
            if let LabEvent::Finished { id: done, result } = next(&events).await {
                assert_eq!(done, id);
                assert!(result.unwrap_err().contains("stopped unexpectedly"));
                break;
            }
        }
    }
}

#[test]
fn a_fifo_or_directory_is_refused_without_opening_it() {
    let dir = std::env::temp_dir().join(format!("kvlm-lab-fifo-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let fifo = dir.join("model.gguf");
    let made = std::process::Command::new("mkfifo")
        .arg(&fifo)
        .status()
        .is_ok_and(|s| s.success());
    if made {
        // Opening a FIFO for reading would block forever; this must return at once.
        assert!(model_facts(&fifo).is_err());
    }
    let _ = std::fs::remove_dir_all(&dir);
}
