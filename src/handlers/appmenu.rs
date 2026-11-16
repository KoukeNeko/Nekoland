//! How programs say where their menus are, so that the menu bar can show them:
//! Qt's over KDE's appmenu protocol (a com.canonical.dbusmenu address), GTK's over its own
//! gtk_shell1 (an org.gtk.Menus menubar and the action groups it uses).

use smithay::reexports::wayland_server::{
    backend::ClientId, protocol::wl_surface::WlSurface, Client, DataInit, Dispatch, DisplayHandle,
    GlobalDispatch, New,
};
use wayland_protocols_plasma::appmenu::server::{
    org_kde_kwin_appmenu::{self, OrgKdeKwinAppmenu},
    org_kde_kwin_appmenu_manager::{self, OrgKdeKwinAppmenuManager},
};

use crate::{dbus::AppMenuAddress, Nekoland};

#[allow(
    dead_code,
    non_camel_case_types,
    non_upper_case_globals,
    unused_imports,
    clippy::all
)]
pub mod gtk_shell {
    use smithay::reexports::wayland_server;
    use smithay::reexports::wayland_server::protocol::*;

    pub mod __interfaces {
        use smithay::reexports::wayland_server::protocol::__interfaces::*;
        wayland_scanner::generate_interfaces!("protocols/gtk-shell.xml");
    }
    use self::__interfaces::*;

    wayland_scanner::generate_server_code!("protocols/gtk-shell.xml");
}

use gtk_shell::{
    gtk_shell1::{self, GtkShell1},
    gtk_surface1::{self, GtkSurface1},
};

pub fn create_globals(dh: &DisplayHandle) {
    dh.create_global::<Nekoland, OrgKdeKwinAppmenuManager, _>(2, ());
    dh.create_global::<Nekoland, GtkShell1, _>(1, ());
}

impl GlobalDispatch<OrgKdeKwinAppmenuManager, ()> for Nekoland {
    fn bind(
        _state: &mut Self,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<OrgKdeKwinAppmenuManager>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<OrgKdeKwinAppmenuManager, ()> for Nekoland {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _manager: &OrgKdeKwinAppmenuManager,
        request: org_kde_kwin_appmenu_manager::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let org_kde_kwin_appmenu_manager::Request::Create { id, surface } = request {
            data_init.init(id, surface);
        }
    }
}

impl Dispatch<OrgKdeKwinAppmenu, WlSurface> for Nekoland {
    fn request(
        state: &mut Self,
        _client: &Client,
        _appmenu: &OrgKdeKwinAppmenu,
        request: org_kde_kwin_appmenu::Request,
        surface: &WlSurface,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        if let org_kde_kwin_appmenu::Request::SetAddress {
            service_name,
            object_path,
        } = request
        {
            state.set_app_menu_address(
                surface,
                AppMenuAddress::Dbusmenu {
                    bus: service_name,
                    path: object_path,
                },
            );
        }
    }

    fn destroyed(_state: &mut Self, _client: ClientId, _appmenu: &OrgKdeKwinAppmenu, _surface: &WlSurface) {}
}

impl GlobalDispatch<GtkShell1, ()> for Nekoland {
    fn bind(
        _state: &mut Self,
        _dh: &DisplayHandle,
        _client: &Client,
        resource: New<GtkShell1>,
        _global_data: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        let shell = data_init.init(resource, ());
        // None of the old capabilities: menus are not shown in the window instead.
        shell.capabilities(0);
    }
}

impl Dispatch<GtkShell1, ()> for Nekoland {
    fn request(
        _state: &mut Self,
        _client: &Client,
        _shell: &GtkShell1,
        request: gtk_shell1::Request,
        _data: &(),
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let gtk_shell1::Request::GetGtkSurface { gtk_surface, surface } = request {
            data_init.init(gtk_surface, surface);
        }
    }
}

impl Dispatch<GtkSurface1, WlSurface> for Nekoland {
    fn request(
        state: &mut Self,
        _client: &Client,
        _gtk_surface: &GtkSurface1,
        request: gtk_surface1::Request,
        surface: &WlSurface,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            gtk_surface1::Request::SetDbusProperties {
                menubar_path,
                window_object_path,
                application_object_path,
                unique_bus_name,
                ..
            } => {
                // Only a menubar is worth showing; the old separate app menu is not used anymore.
                if let (Some(bus), Some(menubar)) = (unique_bus_name, menubar_path) {
                    state.set_app_menu_address(
                        surface,
                        AppMenuAddress::Gtk {
                            bus,
                            menubar,
                            app: application_object_path.unwrap_or_default(),
                            window: window_object_path.unwrap_or_default(),
                        },
                    );
                }
            }
            gtk_surface1::Request::Present { .. } => {
                if let Some(window) = state.window_for_surface(surface) {
                    state.focus_window(&window);
                }
            }
            _ => {}
        }
    }

    fn destroyed(_state: &mut Self, _client: ClientId, _gtk_surface: &GtkSurface1, _surface: &WlSurface) {}
}
