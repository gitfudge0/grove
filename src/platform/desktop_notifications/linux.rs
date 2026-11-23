//! Freedesktop desktop notifications on the session bus.

use std::{collections::HashMap, time::Duration};

use futures::{channel::mpsc, FutureExt as _, StreamExt as _};
use zbus::{names::UniqueName, zvariant::Value, Proxy};

const SERVICE: &str = "org.freedesktop.Notifications";
const PATH: &str = "/org/freedesktop/Notifications";
const DEFAULT_ACTION: &str = "default";

enum Command {
    Show {
        id: u64,
        title: &'static str,
        body: String,
    },
    Close(u64),
}

pub(crate) struct DesktopNotifications {
    activated: mpsc::UnboundedSender<u64>,
    commands: Option<mpsc::UnboundedSender<Command>>,
}

impl DesktopNotifications {
    pub(crate) fn new(activated: mpsc::UnboundedSender<u64>) -> Self {
        Self {
            activated,
            commands: None,
        }
    }

    pub(crate) fn show(&mut self, id: u64, title: &'static str, body: String) {
        if self.commands.is_none() {
            let (sender, receiver) = mpsc::unbounded();
            let activated = self.activated.clone();
            match std::thread::Builder::new()
                .name("desktop-notifications".into())
                .spawn(move || futures::executor::block_on(run(receiver, activated)))
            {
                Ok(_) => self.commands = Some(sender),
                Err(error) => {
                    tracing::debug!(%error, "could not start desktop notification worker");
                }
            }
        }
        if let Some(commands) = &self.commands {
            let _ = commands.unbounded_send(Command::Show { id, title, body });
        }
    }

    pub(crate) fn close(&mut self, id: u64) {
        if let Some(commands) = &self.commands {
            let _ = commands.unbounded_send(Command::Close(id));
        }
    }
}

async fn run(
    mut commands: mpsc::UnboundedReceiver<Command>,
    activated: mpsc::UnboundedSender<u64>,
) {
    while let Some(command) = commands.next().await {
        if matches!(command, Command::Close(_)) {
            continue;
        }
        match serve(command, &mut commands, &activated).await {
            Ok(()) => return,
            Err(error) => tracing::debug!(%error, "desktop notification bus unavailable"),
        }
    }
}

async fn serve(
    first: Command,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    activated: &mpsc::UnboundedSender<u64>,
) -> zbus::Result<()> {
    let connection = zbus::connection::Builder::session()?
        .method_timeout(Duration::from_secs(3))
        .build()
        .await?;
    let proxy = Proxy::new(&connection, SERVICE, PATH, SERVICE).await?;
    // Subscribe before sending anything so an immediate click is still captured.
    let mut signals = proxy.receive_all_signals().await?.fuse();
    let mut posted = HashMap::new();
    handle_command(first, &proxy, &mut posted).await;

    loop {
        futures::select! {
            command = commands.next().fuse() => {
                let Some(command) = command else { return Ok(()) };
                handle_command(command, &proxy, &mut posted).await;
            }
            signal = signals.next() => {
                let signal = signal.ok_or_else(|| {
                    std::io::Error::new(std::io::ErrorKind::BrokenPipe, "session bus disconnected")
                })?;
                handle_signal(&signal, &mut posted, activated);
            }
        }
    }
}

struct Posted {
    owner: UniqueName<'static>,
    id: u32,
}

async fn handle_command(command: Command, proxy: &Proxy<'_>, posted: &mut HashMap<u64, Posted>) {
    let result = match command {
        Command::Show { id, title, body } => {
            post(proxy, title, &body).await.map(|notification| {
                // Daemons can reuse numeric IDs after a service restart.
                posted.retain(|_, item| item.owner == notification.owner);
                posted.insert(id, notification);
            })
        }
        Command::Close(id) => {
            let Some(item) = posted.remove(&id) else {
                return;
            };
            proxy
                .connection()
                .call_method(
                    Some(item.owner),
                    PATH,
                    Some(SERVICE),
                    "CloseNotification",
                    &(item.id,),
                )
                .await
                .map(|_| ())
        }
    };
    if let Err(error) = result {
        tracing::debug!(%error, "desktop notification request failed");
    }
}

async fn post(proxy: &Proxy<'_>, title: &str, body: &str) -> zbus::Result<Posted> {
    let reply = proxy.call_method("GetCapabilities", &()).await?;
    let owner = reply
        .header()
        .sender()
        .ok_or(zbus::Error::InvalidReply)?
        .to_owned();
    let capability_body = reply.body();
    let capabilities: Vec<&str> = capability_body.deserialize()?;
    let actions: &[&str] = if capabilities.contains(&"actions") {
        &[DEFAULT_ACTION, "Open in Grove"]
    } else {
        &[]
    };
    let body = if capabilities.contains(&"body-markup") {
        std::borrow::Cow::Owned(
            body.replace('&', "&amp;")
                .replace('<', "&lt;")
                .replace('>', "&gt;"),
        )
    } else {
        std::borrow::Cow::Borrowed(body)
    };
    let hints = HashMap::from([("desktop-entry", Value::from("grove"))]);
    let reply = proxy
        .connection()
        .call_method(
            Some(owner.as_str()),
            PATH,
            Some(SERVICE),
            "Notify",
            &(
                "Grove",
                0_u32,
                "grove",
                title,
                body.as_ref(),
                actions,
                hints,
                -1_i32,
            ),
        )
        .await?;
    Ok(Posted {
        owner,
        id: reply.body().deserialize()?,
    })
}

fn handle_signal(
    signal: &zbus::Message,
    posted: &mut HashMap<u64, Posted>,
    activated: &mpsc::UnboundedSender<u64>,
) {
    let header = signal.header();
    let (Some(owner), Some(member)) = (header.sender(), header.member()) else {
        return;
    };
    let body = signal.body();
    let (id, is_activation) = match member.as_str() {
        "ActionInvoked" => match body.deserialize::<(u32, &str)>() {
            Ok((id, DEFAULT_ACTION)) => (id, true),
            _ => return,
        },
        "NotificationClosed" => match body.deserialize::<(u32, u32)>() {
            Ok((id, _)) => (id, false),
            Err(_) => return,
        },
        _ => return,
    };
    if let Some(&grove_id) = posted
        .iter()
        .find_map(|(grove_id, item)| (item.id == id && item.owner == *owner).then_some(grove_id))
    {
        if is_activation {
            let _ = activated.unbounded_send(grove_id);
        } else {
            posted.remove(&grove_id);
        }
    }
}
