package main

import "os"

// defaultSocket is where Nekoland's service, nekoland-vpnd.service, listens
// (RuntimeDirectory=nekoland-vpn), and where Settings looks for it.
const defaultSocket = "/run/nekoland-vpn/vpnd.sock"

const (
	// systemdRunDir is there when systemd is the init of the system, and
	// openrcRunDir when OpenRC is.
	systemdRunDir = "/run/systemd/system"
	openrcRunDir  = "/run/openrc"
)

// isDirectory reports whether path is a directory; the tests replace it.
var isDirectory = func(path string) bool {
	info, err := os.Stat(path)
	return err == nil && info.IsDir()
}

// checkSocketPath accepts any path: whatever is not a socket fails when it is
// connected to, with the reason of the OS.
func checkSocketPath(string) error { return nil }

// notRunningHint points at the service when the production daemon is the one
// that cannot be reached, with the command of the init that runs it; a daemon at
// another socket was started by hand.
func notRunningHint(socket string) string {
	if socket != defaultSocket {
		return ""
	}
	switch {
	case isDirectory(systemdRunDir):
		return `; run "systemctl status nekoland-vpnd"`
	case isDirectory(openrcRunDir):
		return `; run "rc-service plaitwayd status"`
	}
	return ""
}
