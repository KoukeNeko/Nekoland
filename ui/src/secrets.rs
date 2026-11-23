//! Passwords in the keyring: the Secret Service's items, found by the attributes another
//! program's libsecret schema gives them, so either program reads what the other kept. Spoken
//! over D-Bus, as libsecret does, with the plain session it uses.
// Files and Settings each use part of it.
#![allow(dead_code)]

use std::collections::HashMap;

use gtk::prelude::*;
use gtk::{gio, glib, glib::variant::ObjectPath};

const SERVICE: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE_IFACE: &str = "org.freedesktop.Secret.Service";
const ITEM_IFACE: &str = "org.freedesktop.Secret.Item";
const PROMPT_IFACE: &str = "org.freedesktop.Secret.Prompt";
/// The keyring that logging in unlocks, with the login's password.
const LOGIN: &str = "/org/freedesktop/secrets/collection/login";
/// What the service answers for "no object": no prompt needed, no collection.
const NONE: &str = "/";

/// An item's attributes, `xdg:schema` among them.
pub type Attributes = HashMap<String, String>;

fn unexpected(what: &str) -> glib::Error {
    glib::Error::new(gio::IOErrorEnum::InvalidData, &format!("鑰匙圈的回應無法讀取：{what}"))
}

fn path(text: &str) -> Result<ObjectPath, glib::Error> {
    ObjectPath::try_from(text.to_owned()).map_err(|_| unexpected(text))
}

struct Keyring {
    bus: gio::DBusConnection,
    session: ObjectPath,
}

impl Keyring {
    async fn open() -> Result<Keyring, glib::Error> {
        let bus = gio::bus_get_future(gio::BusType::Session).await?;
        // (sv): the plain algorithm takes an empty string as its input.
        let input = "".to_variant();
        let reply = call(&bus, SERVICE_PATH, SERVICE_IFACE, "OpenSession", ("plain", input).to_variant()).await?;
        let session = reply.child_value(1).str().map(path).ok_or_else(|| unexpected("OpenSession"))??;
        Ok(Keyring { bus, session })
    }

    async fn close(self) {
        // The service drops a session with the connection anyway; this only frees it sooner.
        let ended = call(&self.bus, self.session.as_str(), "org.freedesktop.Secret.Session", "Close", ().to_variant()).await;
        if let Err(err) = ended {
            eprintln!("keyring session not closed: {err}");
        }
    }

    /// The items, unlocked: the keyring asks for its password when it is locked.
    async fn items(&self, attributes: &Attributes) -> Result<Vec<ObjectPath>, glib::Error> {
        let (mut unlocked, mut locked) = self.search(attributes).await?;
        // gnome-keyring finds nothing, not even locked items, in a keyring it has not unlocked
        // since it started, as after an automatic login: unlocked, it finds them.
        if unlocked.is_empty() && locked.is_empty() {
            // Not default_collection, which would make a keyring where there is none.
            let alias = call(&self.bus, SERVICE_PATH, SERVICE_IFACE, "ReadAlias", ("default",).to_variant()).await?;
            let collection = match alias.child_value(0).str() {
                Some(NONE) => return Ok(Vec::new()),
                Some(collection) => path(collection)?,
                None => return Err(unexpected("ReadAlias")),
            };
            let state = call(
                &self.bus,
                collection.as_str(),
                "org.freedesktop.DBus.Properties",
                "Get",
                ("org.freedesktop.Secret.Collection", "Locked").to_variant(),
            )
            .await?;
            if state.child_value(0).as_variant().and_then(|locked| locked.get::<bool>()) == Some(true) {
                self.unlock(vec![collection]).await?;
                (unlocked, locked) = self.search(attributes).await?;
            }
        }
        if !locked.is_empty() {
            self.unlock(locked.clone()).await?;
            unlocked.extend(locked);
        }
        Ok(unlocked)
    }

    async fn search(&self, attributes: &Attributes) -> Result<(Vec<ObjectPath>, Vec<ObjectPath>), glib::Error> {
        let reply = call(&self.bus, SERVICE_PATH, SERVICE_IFACE, "SearchItems", (attributes,).to_variant()).await?;
        reply.get().ok_or_else(|| unexpected("SearchItems"))
    }

    async fn unlock(&self, objects: Vec<ObjectPath>) -> Result<(), glib::Error> {
        let reply = call(&self.bus, SERVICE_PATH, SERVICE_IFACE, "Unlock", (objects,).to_variant()).await?;
        let prompt = reply.child_value(1).str().map(str::to_owned).ok_or_else(|| unexpected("Unlock"))?;
        self.prompt(&prompt).await.map(drop)
    }

    /// Shows the keyring's own prompt, when it needs one, and gives what it completed with.
    async fn prompt(&self, prompt: &str) -> Result<Option<glib::Variant>, glib::Error> {
        if prompt == NONE {
            return Ok(None);
        }
        let (sender, receiver) = async_channel::bounded(1);
        let subscription = self.bus.signal_subscribe(
            Some(SERVICE),
            Some(PROMPT_IFACE),
            Some("Completed"),
            Some(prompt),
            None,
            gio::DBusSignalFlags::NONE,
            move |_, _, _, _, _, completed| {
                // Only the first completion is waited for; a second has nowhere to go.
                let _ = sender.try_send(completed.clone());
            },
        );
        let shown = call(&self.bus, prompt, PROMPT_IFACE, "Prompt", ("",).to_variant()).await;
        let completed = match shown {
            Ok(_) => receiver.recv().await.map_err(|_| unexpected("Prompt")),
            Err(err) => Err(err),
        };
        self.bus.signal_unsubscribe(subscription);
        let completed = completed?;
        if completed.child_value(0).get::<bool>() != Some(false) {
            return Err(glib::Error::new(gio::IOErrorEnum::FailedHandled, "已取消"));
        }
        Ok(completed.child_value(1).as_variant())
    }

    async fn default_collection(&self) -> Result<ObjectPath, glib::Error> {
        let reply = call(&self.bus, SERVICE_PATH, SERVICE_IFACE, "ReadAlias", ("default",).to_variant()).await?;
        match reply.child_value(0).str() {
            Some(NONE) => self.create_default().await,
            Some(collection) => path(collection),
            None => Err(unexpected("ReadAlias")),
        }
    }

    /// A first keyring, as libsecret makes one: the keyring asks for its new password.
    async fn create_default(&self) -> Result<ObjectPath, glib::Error> {
        let properties = HashMap::from([(
            "org.freedesktop.Secret.Collection.Label".to_owned(),
            "Default keyring".to_variant(),
        )]);
        let reply = call(&self.bus, SERVICE_PATH, SERVICE_IFACE, "CreateCollection", (properties, "default").to_variant()).await?;
        let (collection, prompt) = (reply.child_value(0), reply.child_value(1));
        match (collection.str(), prompt.str()) {
            (Some(NONE), Some(prompt)) => {
                let made = self.prompt(prompt).await?.ok_or_else(|| unexpected("CreateCollection"))?;
                made.str().map(path).ok_or_else(|| unexpected("CreateCollection"))?
            }
            (Some(collection), _) => path(collection),
            _ => Err(unexpected("CreateCollection")),
        }
    }
}

async fn call(
    bus: &gio::DBusConnection,
    object: &str,
    interface: &str,
    method: &str,
    parameters: glib::Variant,
) -> Result<glib::Variant, glib::Error> {
    bus.call_future(
        Some(SERVICE),
        object,
        interface,
        method,
        Some(&parameters),
        None,
        gio::DBusCallFlags::NONE,
        -1,
    )
    .await
}

/// Whether anything is kept under the attributes, without unlocking the keyring to say so. A
/// keyring that is announced but does not answer soon counts as keeping nothing.
pub async fn has(attributes: &Attributes) -> Result<bool, glib::Error> {
    let bus = gio::bus_get_future(gio::BusType::Session).await?;
    let reply = bus
        .call_future(
            Some(SERVICE),
            SERVICE_PATH,
            SERVICE_IFACE,
            "SearchItems",
            Some(&(attributes,).to_variant()),
            None,
            gio::DBusCallFlags::NONE,
            5_000,
        )
        .await?;
    let (unlocked, locked): (Vec<ObjectPath>, Vec<ObjectPath>) =
        reply.get().ok_or_else(|| unexpected("SearchItems"))?;
    Ok(!unlocked.is_empty() || !locked.is_empty())
}

/// The password kept under the attributes, or none when none was kept.
pub async fn lookup(attributes: &Attributes) -> Result<Option<String>, glib::Error> {
    let keyring = Keyring::open().await?;
    let found = async {
        let Some(item) = keyring.items(attributes).await?.into_iter().next() else {
            return Ok(None);
        };
        let reply = call(
            &keyring.bus,
            item.as_str(),
            ITEM_IFACE,
            "GetSecret",
            (keyring.session.clone(),).to_variant(),
        )
        .await?;
        // ((session, parameters, value, content type))
        let value: Vec<u8> = reply
            .child_value(0)
            .child_value(2)
            .get()
            .ok_or_else(|| unexpected("GetSecret"))?;
        String::from_utf8(value).map(Some).map_err(|_| unexpected("GetSecret"))
    }
    .await;
    keyring.close().await;
    found
}

/// Changes the login keyring's password from `old` to `new`, so that logging in keeps unlocking
/// it once the login's own password is changed. Without a login keyring there is nothing to do.
pub async fn change_login_password(old: &str, new: &str) -> Result<(), glib::Error> {
    let keyring = Keyring::open().await?;
    let changed = async {
        let login = path(LOGIN)?;
        let properties = (SERVICE_IFACE, "Collections").to_variant();
        let properties_iface = "org.freedesktop.DBus.Properties";
        let reply = call(&keyring.bus, SERVICE_PATH, properties_iface, "Get", properties).await?;
        let collections: Vec<ObjectPath> = reply
            .child_value(0)
            .as_variant()
            .and_then(|collections| collections.get())
            .ok_or_else(|| unexpected("Collections"))?;
        if !collections.contains(&login) {
            return Ok(());
        }
        let secret =
            |text: &str| (keyring.session.clone(), Vec::<u8>::new(), text.as_bytes().to_vec(), "text/plain");
        // gnome-keyring's own call, which its password dialogs and PAM's password change use.
        let change = (login, secret(old), secret(new)).to_variant();
        let interface = "org.gnome.keyring.InternalUnsupportedGuiltRiddenInterface";
        call(&keyring.bus, SERVICE_PATH, interface, "ChangeWithMasterPassword", change).await.map(drop)
    }
    .await;
    keyring.close().await;
    changed
}

/// Keeps a password in the default keyring under the attributes and `label`, or forgets it
/// when it is empty.
pub async fn store(attributes: &Attributes, label: &str, password: &str) -> Result<(), glib::Error> {
    if password.is_empty() {
        return clear(attributes).await;
    }
    let keyring = Keyring::open().await?;
    let stored = async {
        let collection = keyring.default_collection().await?;
        keyring.unlock(vec![collection.clone()]).await?;
        let properties = HashMap::from([
            ("org.freedesktop.Secret.Item.Label".to_owned(), label.to_variant()),
            ("org.freedesktop.Secret.Item.Attributes".to_owned(), attributes.to_variant()),
        ]);
        let secret = (
            keyring.session.clone(),
            Vec::<u8>::new(),
            password.as_bytes().to_vec(),
            "text/plain",
        );
        let reply = call(
            &keyring.bus,
            collection.as_str(),
            "org.freedesktop.Secret.Collection",
            "CreateItem",
            (properties, secret, true).to_variant(),
        )
        .await?;
        let prompt = reply.child_value(1).str().map(str::to_owned).ok_or_else(|| unexpected("CreateItem"))?;
        keyring.prompt(&prompt).await.map(drop)
    }
    .await;
    keyring.close().await;
    stored
}

/// Forgets what is kept under the attributes.
pub async fn clear(attributes: &Attributes) -> Result<(), glib::Error> {
    let keyring = Keyring::open().await?;
    let cleared = async {
        for item in keyring.items(attributes).await? {
            let reply = call(&keyring.bus, item.as_str(), ITEM_IFACE, "Delete", ().to_variant()).await?;
            let prompt = reply.child_value(0).str().map(str::to_owned).ok_or_else(|| unexpected("Delete"))?;
            keyring.prompt(&prompt).await?;
        }
        Ok(())
    }
    .await;
    keyring.close().await;
    cleared
}
