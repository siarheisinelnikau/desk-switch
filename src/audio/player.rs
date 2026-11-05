use super::{link::Link, playout::Mix, stream};
use crate::config::{Identity, Key};
use anyhow::{Context, Result, bail};
use std::{
    io::{self, BufRead},
    net::SocketAddr,
    sync::{Arc, Mutex},
    thread,
    time::Duration,
};
use tokio::{
    net::TcpStream,
    sync::mpsc,
    time::{sleep, timeout},
};

#[derive(Clone)]
struct Player {
    address: SocketAddr,
    identity: Arc<Identity>,
    peer: Key,
    output: Option<String>,
    mix: Arc<Mutex<Mix>>,
    status: Arc<Mutex<String>>,
}

impl Player {
    fn status(&self, text: String) {
        let mut status = self.status.lock().unwrap();
        if *status != text {
            crate::lifecycle::report(crate::lifecycle::Event::Audio {
                playing: text.starts_with("playing on "),
                message: text.clone(),
            });
            println!("Remote audio: {text}");
            *status = text;
        }
    }

    async fn session(&self) -> Result<()> {
        let socket = timeout(Duration::from_secs(2), TcpStream::connect(self.address))
            .await
            .context("connecting to the Remote audio port timed out")??;
        let mut link = Link::open(socket, &self.identity, &self.peer, true).await?;
        let mut renderer =
            super::output::Renderer::start(self.output.clone(), Arc::clone(&self.mix))?;
        let output = match (&mut renderer.ready).await {
            Ok(output) => output,
            Err(_) => {
                (&mut renderer.done)
                    .await
                    .context("local audio worker stopped during startup")??;
                bail!("local audio output did not start");
            }
        };
        self.status(format!("paired; waiting for samples on {output}"));
        let mut started = false;
        tokio::select! {
            result = stream::receive(&mut link, &renderer.queue, |_| {
                if !started {
                    self.status(format!("playing on {output}"));
                    started = true;
                }
            }) => result,
            result = &mut renderer.done => {
                result.context("local audio worker stopped")??;
                bail!("local audio output stopped");
            }
        }
    }

    async fn reconnect(self) {
        loop {
            if let Err(error) = self.session().await {
                self.status(format!("reconnecting ({error:#})"));
            }
            sleep(Duration::from_secs(1)).await;
        }
    }
}

pub async fn play(
    address: SocketAddr,
    identity: Identity,
    peer: Key,
    output: Option<String>,
    mix: Mix,
) -> Result<()> {
    let player = Player {
        address,
        identity: Arc::new(identity),
        peer,
        output,
        mix: Arc::new(Mutex::new(mix)),
        status: Arc::new(Mutex::new(String::new())),
    };
    let (send, mut commands) = mpsc::channel(8);
    thread::Builder::new()
        .name("Desk Switch audio console".into())
        .spawn(move || {
            for line in io::stdin().lock().lines() {
                let Ok(line) = line else { break };
                if send.blocking_send(line).is_err() {
                    break;
                }
            }
        })?;
    println!("Audio commands: on, off, volume 0–100, mute, unmute, status, quit");
    player.status(format!("connecting to {address}"));
    let mut task = Some(tokio::spawn(player.clone().reconnect()));
    let shutdown = tokio::signal::ctrl_c();
    tokio::pin!(shutdown);
    let mut read_commands = true;
    let signal = loop {
        tokio::select! {
            result = &mut shutdown => break result,
            command = commands.recv(), if read_commands => {
                let Some(command) = command else {
                    if crate::lifecycle::managed() { break Ok(()); }
                    read_commands = false;
                    continue;
                };
                match command.split_whitespace().collect::<Vec<_>>().as_slice() {
                    ["on"] if task.is_none() => {
                        player.status(format!("connecting to {address}"));
                        task = Some(tokio::spawn(player.clone().reconnect()));
                    }
                    ["on"] => println!("Remote audio is already enabled."),
                    ["off"] => {
                        stop(&mut task).await;
                        player.status("off".into());
                    }
                    ["mute"] => {
                        player.mix.lock().unwrap().muted = true;
                        println!("Remote audio muted.");
                    }
                    ["unmute"] => {
                        player.mix.lock().unwrap().muted = false;
                        println!("Remote audio unmuted.");
                    }
                    ["volume", value] => match super::volume(value) {
                        Ok(volume) => {
                            player.mix.lock().unwrap().volume = volume;
                            println!("Remote audio volume {volume}%.");
                        }
                        Err(error) => eprintln!("{error}"),
                    },
                    ["status"] => {
                        let mix = *player.mix.lock().unwrap();
                        println!("Remote audio: {} | {}%{}", player.status.lock().unwrap(), mix.volume,
                            if mix.muted { " | muted" } else { "" });
                    }
                    ["quit"] => break Ok(()),
                    [] => {}
                    _ => eprintln!("Use on, off, volume 0–100, mute, unmute, status, or quit."),
                }
            }
        }
    };
    stop(&mut task).await;
    player.status("off".into());
    Ok(signal?)
}

async fn stop(task: &mut Option<tokio::task::JoinHandle<()>>) {
    if let Some(task) = task.take() {
        task.abort();
        let _ = task.await;
    }
}
