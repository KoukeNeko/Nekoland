package main

const (
	// Where the systemd unit keeps the state of the daemon and its run files:
	// StateDirectory=nekoland-vpn (mode 0700) and RuntimeDirectory=nekoland-vpn.
	productionStateDir = "/var/lib/nekoland-vpn"
	productionRunDir   = "/run/nekoland-vpn"
)

// productionLocations has no log file: the daemon logs to stderr, which systemd
// sends to the journal.
func productionLocations() (locations, error) {
	return locations{stateDir: productionStateDir, runDir: productionRunDir}, nil
}
