//! One thread advances stub jobs between stages. The UI thread does not
//! develop. Real pipeline work replaces the body of [`crate::jobs::ContractStub::tick`].

use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use tauri::{AppHandle, Emitter, Manager};

use crate::contract::ContractEvent;
use crate::state::{self, AppState, lock};

pub fn spawn(app: AppHandle) {
    let (sender, receiver) = mpsc::channel();
    *lock(&app.state::<AppState>().wake) = Some(sender);
    thread::Builder::new()
        .name("rawstock-jobs".to_string())
        .spawn(move || {
            loop {
                let state = app.state::<AppState>();
                if state.stopping.load(std::sync::atomic::Ordering::Relaxed) {
                    break;
                }
                let (events, running) = {
                    let mut stub = lock(&state.stub);
                    let events = stub.tick();
                    let running = stub.has_running();
                    (events, running)
                };
                emit(&app, &events);
                if running {
                    let _ = receiver.recv_timeout(Duration::from_millis(40));
                } else if receiver.recv().is_err() {
                    break;
                }
            }
        })
        .expect("spawn the develop job thread");
}

pub fn emit(app: &AppHandle, events: &[ContractEvent]) {
    for event in events {
        let sent = match event {
            ContractEvent::Progress(payload) => app.emit("job:progress", payload),
            ContractEvent::Done(payload) => app.emit("job:done", payload),
            ContractEvent::Cancelled(payload) => app.emit("job:cancelled", payload),
            ContractEvent::Failed(payload) => app.emit("job:failed", payload),
            ContractEvent::ThumbnailReady(payload) => app.emit("thumbnail:ready", payload),
            ContractEvent::CacheReady(payload) => app.emit("cache:ready", payload),
            ContractEvent::CacheEvicted(payload) => app.emit("cache:evicted", payload),
        };
        if let Err(error) = sent {
            state::log_detail(
                &app.state::<AppState>().log_dir,
                &format!("emit {} failed: {error}", event.name()),
            );
        }
    }
}
