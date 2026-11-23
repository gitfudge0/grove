//! Windows desktop toasts use the `AppUserModelID` registered by the MSI.
#![allow(unsafe_code)]

use std::collections::HashMap;
use std::sync::mpsc;

use futures::channel::mpsc::UnboundedSender;
use windows::core::{Result, HSTRING};
use windows::Data::Xml::Dom::XmlDocument;
use windows::Foundation::TypedEventHandler;
use windows::Win32::System::WinRT::{RoInitialize, RoUninitialize, RO_INIT_MULTITHREADED};
use windows::UI::Notifications::{
    NotificationSetting, ToastFailedEventArgs, ToastNotification, ToastNotificationManager,
    ToastNotifier,
};

const APP_ID: &str = "dev.grove.Grove";

enum Command {
    Show {
        id: u64,
        title: &'static str,
        body: String,
    },
    Close(u64),
}

pub(crate) struct DesktopNotifications {
    activated: Option<UnboundedSender<u64>>,
    commands: Option<mpsc::Sender<Command>>,
}

impl DesktopNotifications {
    pub(crate) fn new(activated: UnboundedSender<u64>) -> Self {
        Self {
            activated: Some(activated),
            commands: None,
        }
    }

    pub(crate) fn show(&mut self, id: u64, title: &'static str, body: String) {
        if let Some(activated) = self.activated.take() {
            let (tx, rx) = mpsc::channel();
            match std::thread::Builder::new()
                .name("grove-desktop-notifications".into())
                .spawn(move || {
                    if let Err(error) = serve(rx, &activated) {
                        tracing::debug!(%error, "Windows desktop notifications unavailable");
                    }
                }) {
                Ok(_) => self.commands = Some(tx),
                Err(error) => tracing::debug!(%error, "Could not start Windows notifications"),
            }
        }
        if let Some(commands) = &self.commands {
            let _ = commands.send(Command::Show { id, title, body });
        }
    }

    pub(crate) fn close(&mut self, id: u64) {
        if let Some(commands) = &self.commands {
            let _ = commands.send(Command::Close(id));
        }
    }
}

struct WinRt;

impl WinRt {
    fn initialize() -> Result<Self> {
        // SAFETY: This is a dedicated thread. The returned guard balances a
        // successful initialization on that thread after all WinRT objects drop.
        unsafe { RoInitialize(RO_INIT_MULTITHREADED)? };
        Ok(Self)
    }
}

impl Drop for WinRt {
    fn drop(&mut self) {
        // SAFETY: The guard is private to the thread that initialized WinRT.
        unsafe { RoUninitialize() };
    }
}

struct Toast {
    notification: ToastNotification,
    activation: i64,
    failure: Option<i64>,
}

impl Toast {
    fn show(
        notifier: &ToastNotifier,
        activated: UnboundedSender<u64>,
        id: u64,
        title: &str,
        body: &str,
    ) -> Result<Option<Self>> {
        if notifier.Setting()? != NotificationSetting::Enabled {
            return Ok(None);
        }

        let content = XmlDocument::new()?;
        content.LoadXml(&HSTRING::from(
            "<toast launch=\"open\"><visual><binding template=\"ToastGeneric\"><text/><text/></binding></visual></toast>",
        ))?;
        let text_nodes = content.GetElementsByTagName(&HSTRING::from("text"))?;
        text_nodes
            .Item(0)?
            .AppendChild(&content.CreateTextNode(&HSTRING::from(title))?)?;
        text_nodes
            .Item(1)?
            .AppendChild(&content.CreateTextNode(&HSTRING::from(body))?)?;
        let notification = ToastNotification::CreateToastNotification(&content)?;
        let activation = notification.Activated(&TypedEventHandler::new(move |_, _| {
            let _ = activated.unbounded_send(id);
            Ok(())
        }))?;
        let failure = notification
            .Failed(&TypedEventHandler::<ToastNotification, ToastFailedEventArgs>::new(
                |_, args| {
                    if let Some(args) = args.as_ref() {
                        tracing::debug!(code = ?args.ErrorCode(), "Windows did not deliver a Grove toast");
                    }
                    Ok(())
                },
            ))
            .ok();
        let toast = Self {
            notification,
            activation,
            failure,
        };
        notifier.Show(&toast.notification)?;
        Ok(Some(toast))
    }
}

impl Drop for Toast {
    fn drop(&mut self) {
        let _ = self.notification.RemoveActivated(self.activation);
        if let Some(failure) = self.failure {
            let _ = self.notification.RemoveFailed(failure);
        }
    }
}

fn serve(commands: mpsc::Receiver<Command>, activated: &UnboundedSender<u64>) -> Result<()> {
    let _winrt = WinRt::initialize()?;
    let notifier = ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(APP_ID))?;
    let mut visible = HashMap::<u64, Toast>::new();
    for command in commands {
        match command {
            Command::Show { id, title, body } => {
                if visible.contains_key(&id) {
                    continue;
                }
                match Toast::show(&notifier, activated.clone(), id, title, &body) {
                    Ok(Some(toast)) => {
                        visible.insert(id, toast);
                    }
                    Ok(None) => {}
                    Err(error) => tracing::debug!(%error, "Could not show Windows toast"),
                }
            }
            Command::Close(id) => {
                if let Some(toast) = visible.remove(&id) {
                    let _ = notifier.Hide(&toast.notification);
                }
            }
        }
    }
    for (_, toast) in visible {
        let _ = notifier.Hide(&toast.notification);
    }
    Ok(())
}
