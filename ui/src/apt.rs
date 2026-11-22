//! apt-get as Nekoland's programs run it, Market to install and remove what it offers and
//! Settings what its pages need.

/// apt-get as Market runs it, with nothing to answer: a configuration file changed
/// both here and in the new version keeps the one here, as dpkg would otherwise ask on a
/// closed input and leave the package half set up.
/// Its status lines go to what it prints, where Market's `run_one` reads them for the progress.
pub const APT_GET: [&str; 12] = [
    "env",
    "DEBIAN_FRONTEND=noninteractive",
    "apt-get",
    "-y",
    "-o",
    "Dpkg::Options::=--force-confdef",
    "-o",
    "Dpkg::Options::=--force-confold",
    "-o",
    APT_LOCK_WAIT,
    "-o",
    "APT::Status-Fd=1",
];

/// apt fails at once while another apt holds its lock, as a second install started before
/// the first ends would; this has it wait for that one instead, for up to ten minutes.
pub const APT_LOCK_WAIT: &str = "DPkg::Lock::Timeout=600";
