use super::files::FileAction;
use super::*;
use crate::task::{Job, Sink};
use tokio::sync::mpsc::{UnboundedReceiver, UnboundedSender, unbounded_channel};

/// Drives the app like the core thread does, against the demo services.
pub(crate) struct Harness {
    pub(crate) app: App,
    pub(crate) tx: UnboundedSender<Msg>,
    pub(crate) rx: UnboundedReceiver<Msg>,
    pub(crate) effects: Vec<Effect>,
}
impl Harness {
    /// The app booted on the demo services, its first tasks running.
    pub(crate) fn demo() -> Self {
        let (app, task) = App::boot(true);
        let (tx, rx) = unbounded_channel();
        let mut h = Harness {
            app,
            tx,
            rx,
            effects: vec![],
        };
        h.run(task);
        h
    }
    pub(crate) fn run(&mut self, task: Task<Msg>) {
        for job in task.0 {
            match job {
                Job::Effect(effect) => self.effects.push(effect),
                Job::Future(future) => {
                    let tx = self.tx.clone();
                    tokio::spawn(async move {
                        let _ = tx.send(future.await);
                    });
                }
                Job::Subscribe(start) => {
                    let tx = self.tx.clone();
                    tokio::spawn(start(Sink::new(move |msg| {
                        let _ = tx.send(msg);
                    })));
                }
                Job::Done(msg) => {
                    let _ = self.tx.send(msg);
                }
            }
        }
    }
    pub(crate) fn send(&mut self, msg: Msg) {
        let task = self.app.update(msg);
        self.run(task);
    }
    pub(crate) fn file(&mut self, action: FileAction) {
        self.send(Msg::Files(FilesMsg::Action(action)));
    }
    pub(crate) async fn until(&mut self, what: &str, done: impl Fn(&App) -> bool) {
        let deadline = std::time::Instant::now() + Duration::from_secs(15);
        while !done(&self.app) {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            let msg = tokio::time::timeout(left, self.rx.recv())
                .await
                .unwrap_or_else(|_| panic!("timed out waiting for {what}"))
                .expect("channel open");
            let task = self.app.update(msg);
            self.run(task);
        }
    }
}
