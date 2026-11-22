//! The passwords of Hamasen's servers in the keyring: the Secret Service's items under
//! Hamasen's libsecret schema, found by the server's id, so either program reads what the
//! other kept. Spoken over D-Bus, as libsecret does, with the plain session it uses.

use std::collections::HashMap;

use gtk::prelude::*;
use gtk::{gio, glib, glib::variant::ObjectPath};

const SCHEMA: &str = "app.hamasen.Hamasen";
const LABEL: &str = "Hamasen";
const SERVICE: &str = "org.freedesktop.secrets";
const SERVICE_PATH: &str = "/org/freedesktop/secrets";
const SERVICE_IFACE: &str = "org.freedesktop.Secret.Service";
const ITEM_IFACE: &str = "org.freedesktop.Secret.Item";
const PROMPT_IFACE: &str = "org.freedesktop.Secret.Prompt";
/// What the service answers for "no object": no prompt needed, no collection.
const NONE: &str = "/";

fn attributes(id: &str) -> HashMap<String, String> {
    HashMap::from([
        ("xdg:schema".to_owned(), SCHEMA.to_owned()),
        ("server".to_owned(), id.to_owned()),
    ])
}

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
            eprintln!("nekoland-files: keyring session not closed: {err}");
        }
    }

    /// The server's items, unlocked: the keyring asks for its password when it is locked.
    async fn items(&self, id: &str) -> Result<Vec<ObjectPath>, glib::Error> {
        let reply = call(&self.bus, SERVICE_PATH, SERVICE_IFACE, "SearchItems", (attributes(id),).to_variant()).await?;
        let (mut unlocked, locked): (Vec<ObjectPath>, Vec<ObjectPath>) =
            reply.get().ok_or_else(|| unexpected("SearchItems"))?;
        if !locked.is_empty() {
            self.unlock(locked.clone()).await?;
            unlocked.extend(locked);
        }
        Ok(unlocked)
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

/// The server's password, or none when none was kept.
pub async fn lookup(id: &str) -> Result<Option<String>, glib::Error> {
    let keyring = Keyring::open().await?;
    let found = async {
        let Some(item) = keyring.items(id).await?.into_iter().next() else {
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

/// Keeps the server's password in the default keyring, or forgets it when it is empty.
pub async fn store(id: &str, password: &str) -> Result<(), glib::Error> {
    if password.is_empty() {
        return clear(id).await;
    }
    let keyring = Keyring::open().await?;
    let stored = async {
        let collection = keyring.default_collection().await?;
        keyring.unlock(vec![collection.clone()]).await?;
        let properties = HashMap::from([
            ("org.freedesktop.Secret.Item.Label".to_owned(), LABEL.to_variant()),
            ("org.freedesktop.Secret.Item.Attributes".to_owned(), attributes(id).to_variant()),
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

/// Forgets the server's password.
pub async fn clear(id: &str) -> Result<(), glib::Error> {
    let keyring = Keyring::open().await?;
    let cleared = async {
        for item in keyring.items(id).await? {
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
