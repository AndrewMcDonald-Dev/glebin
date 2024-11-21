use std::{
    collections::VecDeque,
    sync::Arc,
    time::{Duration, Instant},
};

use log::{debug, info};
use tokio::{
    net::TcpListener,
    sync::{broadcast, Mutex},
};

use crate::{
    game::GameState,
    server::{handle_client, Message},
};

pub async fn run(listener: TcpListener) {
    let mut state = GameState::new();
    let msg_q = Arc::new(Mutex::new(VecDeque::<Message>::new()));
    let (tx, _rx) = broadcast::channel(10);
    info!("Server started on 127.0.0.1:8080");

    {
        let msg_q = msg_q.clone();
        let tx = tx.clone();
        tokio::spawn(async move {
            loop {
                let (socket, _) = listener.accept().await.unwrap();
                let msg_q = msg_q.clone();
                let tx = tx.clone();
                tokio::spawn(async move {
                    handle_client(socket, msg_q, tx).await;
                });
            }
        });
    }

    let mut time = Instant::now();
    loop {
        // wait until x time passes
        // Currently, the server is set to send updates every 8ms.
        // This is to target 128 updates per second.
        if time.elapsed() > Duration::from_millis(8) {
            let msg_q_copy;
            {
                // grab control over message queue
                let mut msg_q = msg_q.lock().await;

                // copy message queue
                msg_q_copy = msg_q.clone();

                // clear original queue
                msg_q.clear();

                // let go of control over message queue
            }

            // process state with message queue
            state.process_messages(msg_q_copy).unwrap();

            // Send new state to all subscribers.
            let state_string = state.get_state();
            let num_received = tx
                .send(state_string.clone())
                .expect("Could not send message.");
            debug!(
                "{} clients received message: {}",
                num_received, state_string
            );

            // reset timer
            time = Instant::now();
        }
    }
}
