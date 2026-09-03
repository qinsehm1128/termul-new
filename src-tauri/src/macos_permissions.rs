//! macOS privacy (TCC) probing for the settings panel.
//!
//! macOS exposes no single "what am I allowed to do" API. Each resource has its
//! own answer, and several have none at all, so this module deliberately mixes
//! three kinds of evidence and labels which is which:
//!
//! * **Preflight APIs** — `AXIsProcessTrusted`, `CGPreflightScreenCaptureAccess`
//!   and `IOHIDCheckAccess` report status without prompting.
//! * **Permission errors** — opening a file only the Full Disk Access grant can
//!   reach answers the question by failing (or not). Also does not prompt.
//! * **Behavioural probes** — local network access has *no* status API at all
//!   (see Apple TN3179), so the only way to learn the answer is to attempt the
//!   operation. That attempt is what shows the system prompt, which is exactly
//!   what a user who has never been asked needs.
//!
//! Because the third kind has a visible side effect, the caller names the ids it
//! wants probed rather than getting them by default: see the `active` argument
//! of [`collect_report`].

use serde::Serialize;

/// Stable ids shared with the renderer. A probe's id also selects its System
/// Settings pane, so the set is closed on purpose — see [`privacy_pane_url`].
pub const ID_FULL_DISK_ACCESS: &str = "fullDiskAccess";
pub const ID_ACCESSIBILITY: &str = "accessibility";
pub const ID_SCREEN_RECORDING: &str = "screenRecording";
pub const ID_INPUT_MONITORING: &str = "inputMonitoring";
pub const ID_LOCAL_NETWORK: &str = "localNetwork";
pub const ID_DESKTOP_FOLDER: &str = "desktopFolder";
pub const ID_DOCUMENTS_FOLDER: &str = "documentsFolder";
pub const ID_DOWNLOADS_FOLDER: &str = "downloadsFolder";

/// Ids whose probe can make macOS show a permission prompt.
pub const ACTIVE_PROBE_IDS: &[&str] = &[
    ID_LOCAL_NETWORK,
    ID_DESKTOP_FOLDER,
    ID_DOCUMENTS_FOLDER,
    ID_DOWNLOADS_FOLDER,
];

/// M-13 — the one root in the merge plan that has no migration at all.
///
/// TCC keys every grant on the bundle identifier (plus the code signature).
/// Changing the identifier presents a *different* application to TCC, so every
/// grant the user has already given resets to "not yet asked". There is no
/// supported API to carry a grant from one identifier to another, and writing
/// `TCC.db` directly is blocked by SIP. So this is not a defect to be fixed
/// and not a copy to be scheduled — it is a property of the platform, and the
/// only correct handling is to say so *before* the merge runs rather than let
/// the user discover it as a terminal command dying with no explanation.
///
/// `parse_codesign_output` already reports the neighbouring case in
/// [`SigningIdentity::grants_survive_rebuild`]: an ad-hoc signed build loses
/// its grants on every rebuild. The identifier change loses them once, for
/// everyone.
pub const TCC_GRANTS_RESET_NOTICE: &str = "macOS privacy permissions cannot be \
     migrated. The rename changes this app's bundle identifier, and macOS ties \
     every privacy approval to that identifier, so macOS will ask again the \
     first time something needs one. Nothing is lost — you will simply be \
     prompted, or you can re-approve in System Settings > Privacy & Security.";

/// The privacy categories [`TCC_GRANTS_RESET_NOTICE`] is about, as the
/// `NS*UsageDescription` keys that declare them.
///
/// Kept in step with `Info.plist` by
/// `tcc_reset_categories_match_the_declared_usage_descriptions`: the plist is
/// what macOS actually reads, so it — not this list — is the oracle. A key
/// added there without a line here means the merge notice would understate
/// what the user is about to lose.
pub const TCC_GRANTS_RESET_CATEGORIES: &[&str] = &[
    "NSAppleEventsUsageDescription",
    "NSBluetoothAlwaysUsageDescription",
    "NSCalendarsUsageDescription",
    "NSCameraUsageDescription",
    "NSContactsUsageDescription",
    "NSDesktopFolderUsageDescription",
    "NSDocumentsFolderUsageDescription",
    "NSDownloadsFolderUsageDescription",
    "NSFileProviderDomainUsageDescription",
    "NSLocalNetworkUsageDescription",
    "NSLocationWhenInUseUsageDescription",
    "NSMicrophoneUsageDescription",
    "NSMotionUsageDescription",
    "NSNetworkVolumesUsageDescription",
    "NSPhotoLibraryUsageDescription",
    "NSRemindersUsageDescription",
    "NSRemovableVolumesUsageDescription",
    "NSScreenCaptureUsageDescription",
    "NSSpeechRecognitionUsageDescription",
    "NSSystemAdministrationUsageDescription",
];

/// The M-13 notice, or `None` on a platform that has no TCC.
///
/// The merge entry point renders whatever this returns *before* offering the
/// "start merge" action; a `None` means the platform has nothing to warn about
/// and the entry point must not invent a warning.
#[must_use]
pub fn tcc_grants_reset_notice() -> Option<&'static str> {
    if cfg!(target_os = "macos") {
        Some(TCC_GRANTS_RESET_NOTICE)
    } else {
        None
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum PermissionState {
    Granted,
    Denied,
    /// Probed but the answer is genuinely indeterminate — not a synonym for
    /// denied, and the UI must not present it as one.
    Unknown,
    /// The probe has a visible side effect and the caller did not ask for it.
    NotProbed,
    /// This OS build does not gate the resource at all.
    NotRequired,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionProbe {
    pub id: String,
    pub state: PermissionState,
    /// Raw evidence behind `state` — an errno name, a path, an API return code.
    /// Shown verbatim so a wrong verdict is diagnosable rather than mysterious.
    pub detail: Option<String>,
    /// Whether running this probe can make macOS show a prompt.
    pub active: bool,
}

impl PermissionProbe {
    fn new(id: &str, state: PermissionState, detail: Option<String>) -> Self {
        Self {
            id: id.to_string(),
            state,
            detail,
            active: ACTIVE_PROBE_IDS.contains(&id),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum SigningKind {
    DeveloperId,
    AppleDevelopment,
    Adhoc,
    Unsigned,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SigningIdentity {
    pub kind: SigningKind,
    pub team_id: Option<String>,
    /// TCC remembers a grant against the app's code identity. A certificate
    /// gives that identity a stable designated requirement; ad-hoc and unsigned
    /// builds fall back to the cdhash, which changes on every build — so every
    /// rebuild looks like a different app and the user must grant again.
    pub grants_survive_rebuild: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionReport {
    /// False off macOS; the renderer hides the whole panel in that case.
    pub supported: bool,
    pub os_version: Option<String>,
    pub bundle_id: Option<String>,
    pub signing: Option<SigningIdentity>,
    pub probes: Vec<PermissionProbe>,
}

// ---------------------------------------------------------------------------
// Pure helpers — the parts worth testing on every platform.
// ---------------------------------------------------------------------------

/// Major version from a `sw_vers`-style string such as `"15.3.1"`.
pub fn parse_major_version(version: &str) -> Option<u32> {
    version.split('.').next()?.trim().parse::<u32>().ok()
}

/// Whether this macOS build gates outgoing local-network traffic.
///
/// The restriction arrived in macOS 15 (TN3179). On anything older the resource
/// is simply not protected, which is a different answer from "denied".
pub fn local_network_gate_applies(os_version: &str) -> bool {
    parse_major_version(os_version).is_some_and(|major| major >= 15)
}

/// Darwin errno values the local-network classifier reasons about.
///
/// Spelled out instead of pulled from `libc` so the classifier and its tests
/// build on every host — `libc` is a macOS-only dependency here. These are
/// frozen ABI constants; `errno_constants_match_libc` pins them on macOS.
const EPERM: i32 = 1;
const EACCES: i32 = 13;
const ENETDOWN: i32 = 50;
const ENETUNREACH: i32 = 51;
const EHOSTUNREACH: i32 = 65;

/// Turn a `sendto` result into a verdict.
///
/// A denied app gets `EHOSTUNREACH` — TN3179 documents that specific errno for
/// UDP. `EPERM`/`EACCES` are folded in because a sandbox or firewall denial is
/// still a denial from the user's point of view. Anything else (no route, no
/// interface) says nothing about permission and must stay `Unknown`.
pub fn classify_local_network_probe(sent: isize, errno: i32) -> (PermissionState, Option<String>) {
    if sent >= 0 {
        return (PermissionState::Granted, Some("sendto ok".to_string()));
    }
    match errno {
        EHOSTUNREACH => (PermissionState::Denied, Some("EHOSTUNREACH".to_string())),
        EPERM => (PermissionState::Denied, Some("EPERM".to_string())),
        EACCES => (PermissionState::Denied, Some("EACCES".to_string())),
        ENETDOWN => (PermissionState::Unknown, Some("ENETDOWN".to_string())),
        ENETUNREACH => (PermissionState::Unknown, Some("ENETUNREACH".to_string())),
        other => (PermissionState::Unknown, Some(format!("errno {other}"))),
    }
}

/// Reduce per-path open attempts into the Full Disk Access verdict.
///
/// Each item is a path and the outcome of opening it: `Ok` for a successful
/// read, `Err(kind)` otherwise. Split from the I/O so the walk's one real rule
/// is testable: a `NotFound` proves nothing and must not end the walk, because
/// the per-user TCC database is absent on some macOS versions and stopping
/// there would report `Unknown` on a machine that has a definite answer one
/// path further down.
pub fn resolve_full_disk_access<'a, I>(outcomes: I) -> (PermissionState, Option<String>)
where
    I: IntoIterator<Item = (&'a str, Result<(), std::io::ErrorKind>)>,
{
    let mut last_detail: Option<String> = None;
    for (path, outcome) in outcomes {
        match outcome {
            Ok(()) => return (PermissionState::Granted, Some(format!("read {path}"))),
            Err(std::io::ErrorKind::PermissionDenied) => {
                return (PermissionState::Denied, Some(format!("EACCES {path}")))
            }
            Err(std::io::ErrorKind::NotFound) => continue,
            Err(kind) => last_detail = Some(format!("{path}: {kind:?}")),
        }
    }
    (
        PermissionState::Unknown,
        Some(last_detail.unwrap_or_else(|| "no TCC database found".to_string())),
    )
}

/// Pull the IPv4 default gateway out of `route -n get default` output.
///
/// Only a private-range address is accepted. A full-tunnel VPN also installs a
/// default route, but its peer (Tailscale, a Clash-style TUN on 198.18/15, ...)
/// is not a local network address, and probing it would answer a different
/// question than the one the panel asks. Rejecting it yields `Unknown`, which
/// is the honest answer.
pub fn parse_default_gateway(output: &str) -> Option<[u8; 4]> {
    let raw = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("gateway:"))?
        .trim();

    // `link#30` shows up for interfaces with no next-hop address.
    let mut octets = [0u8; 4];
    let mut parts = raw.split('.');
    for octet in octets.iter_mut() {
        *octet = parts.next()?.trim().parse::<u8>().ok()?;
    }
    if parts.next().is_some() {
        return None;
    }

    let is_private = match octets {
        [10, ..] => true,
        [172, second, ..] => (16..=31).contains(&second),
        [192, 168, ..] => true,
        // Link-local, for a network with no DHCP server.
        [169, 254, ..] => true,
        _ => false,
    };
    is_private.then_some(octets)
}

/// Classify `IOHIDCheckAccess`'s `IOHIDAccessType`.
pub fn classify_hid_access(access: u32) -> PermissionState {
    match access {
        0 => PermissionState::Granted,
        1 => PermissionState::Denied,
        // 2 is kIOHIDAccessTypeUnknown: never asked.
        _ => PermissionState::Unknown,
    }
}

/// Read a signing identity out of `codesign -dv --verbose=2` output.
///
/// `codesign` writes this block to stderr; the caller passes it through
/// unchanged. Order matters: an ad-hoc signature also carries `Authority=`
/// lines in some configurations, so the ad-hoc marker is checked first.
pub fn parse_codesign_output(output: &str) -> SigningIdentity {
    if output.contains("not signed at all") || output.contains("code object is not signed") {
        return SigningIdentity {
            kind: SigningKind::Unsigned,
            team_id: None,
            grants_survive_rebuild: false,
        };
    }

    let team_id = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("TeamIdentifier="))
        .map(str::trim)
        .filter(|value| !value.is_empty() && *value != "not set")
        .map(str::to_string);

    let is_adhoc = output.lines().any(|line| {
        let line = line.trim();
        line == "Signature=adhoc" || (line.starts_with("CodeDirectory") && line.contains("adhoc"))
    });
    if is_adhoc {
        return SigningIdentity {
            kind: SigningKind::Adhoc,
            team_id,
            grants_survive_rebuild: false,
        };
    }

    let authority = output
        .lines()
        .find_map(|line| line.trim().strip_prefix("Authority="))
        .map(str::trim)
        .unwrap_or_default();

    let kind = if authority.starts_with("Developer ID Application") {
        SigningKind::DeveloperId
    } else if authority.starts_with("Apple Development") || authority.starts_with("Mac Developer") {
        SigningKind::AppleDevelopment
    } else {
        SigningKind::Unknown
    };

    let grants_survive_rebuild = matches!(
        kind,
        SigningKind::DeveloperId | SigningKind::AppleDevelopment
    );

    SigningIdentity {
        kind,
        team_id,
        grants_survive_rebuild,
    }
}

/// System Settings deep link for a probe id.
///
/// A closed map rather than a pass-through: the renderer sends an id it already
/// received from this module, so no caller can hand an arbitrary URL to the
/// system opener.
pub fn privacy_pane_url(id: &str) -> Option<&'static str> {
    /// Every pane lives under the same Ventura-and-later extension; only the
    /// anchor differs.
    macro_rules! pane {
        ($anchor:literal) => {
            concat!(
                "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?",
                $anchor
            )
        };
    }
    match id {
        ID_FULL_DISK_ACCESS => Some(pane!("Privacy_AllFiles")),
        ID_ACCESSIBILITY => Some(pane!("Privacy_Accessibility")),
        ID_SCREEN_RECORDING => Some(pane!("Privacy_ScreenCapture")),
        ID_INPUT_MONITORING => Some(pane!("Privacy_ListenEvent")),
        ID_LOCAL_NETWORK => Some(pane!("Privacy_LocalNetwork")),
        ID_DESKTOP_FOLDER | ID_DOCUMENTS_FOLDER | ID_DOWNLOADS_FOLDER => {
            Some(pane!("Privacy_FilesAndFolders"))
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// macOS implementation.
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod imp {
    use super::*;
    use std::path::PathBuf;

    // Preflight entry points. Each is documented as prompt-free, which is why
    // they can run the moment the panel opens.
    #[link(name = "ApplicationServices", kind = "framework")]
    extern "C" {
        /// Returns `Boolean` (unsigned char), so it is read as `u8` — a Rust
        /// `bool` return would be UB for any value outside {0, 1}.
        fn AXIsProcessTrusted() -> u8;
    }

    #[link(name = "CoreGraphics", kind = "framework")]
    extern "C" {
        fn CGPreflightScreenCaptureAccess() -> u8;
    }

    #[link(name = "IOKit", kind = "framework")]
    extern "C" {
        /// `kIOHIDRequestTypeListenEvent` is 1; returns an `IOHIDAccessType`.
        fn IOHIDCheckAccess(request: u32) -> u32;
    }

    const K_IOHID_REQUEST_TYPE_LISTEN_EVENT: u32 = 1;

    fn home_dir() -> Option<PathBuf> {
        std::env::var_os("HOME")
            .filter(|value| !value.is_empty())
            .map(PathBuf::from)
    }

    pub fn os_version() -> Option<String> {
        // `kern.osproductversion` is the marketing version ("15.3"), which is
        // what the TN3179 threshold is expressed in.
        let name = c"kern.osproductversion";
        let mut size: libc::size_t = 0;
        // SAFETY: a null value pointer asks only for the required length.
        let probe = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                std::ptr::null_mut(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if probe != 0 || size == 0 {
            return None;
        }
        let mut buffer = vec![0u8; size];
        // SAFETY: `buffer` holds exactly the length the call above reported.
        let read = unsafe {
            libc::sysctlbyname(
                name.as_ptr(),
                buffer.as_mut_ptr().cast(),
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if read != 0 {
            return None;
        }
        let text = String::from_utf8_lossy(&buffer);
        let trimmed = text.trim_end_matches('\0').trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(trimmed.to_string())
        }
    }

    pub fn signing_identity() -> Option<SigningIdentity> {
        let executable = std::env::current_exe().ok()?;
        // `codesign` reports the block on stderr, not stdout.
        let output = std::process::Command::new("/usr/bin/codesign")
            .arg("-dv")
            .arg("--verbose=2")
            .arg(&executable)
            .output()
            .ok()?;
        let text = String::from_utf8_lossy(&output.stderr);
        Some(parse_codesign_output(&text))
    }

    /// Full Disk Access, decided by whether a TCC database is readable.
    ///
    /// These files are unreadable to every app that lacks the grant and readable
    /// to every app that has it, and opening one never prompts — which is the
    /// whole reason this is the conventional probe.
    ///
    /// Two candidates because the per-user database is not present on every
    /// macOS version (it is absent on 26), while the system one always is. A
    /// missing file proves nothing, so the walk continues past `NotFound` and
    /// only a definite read or a definite denial ends it.
    fn probe_full_disk_access() -> PermissionProbe {
        let mut candidates: Vec<PathBuf> = Vec::new();
        if let Some(home) = home_dir() {
            candidates.push(home.join("Library/Application Support/com.apple.TCC/TCC.db"));
        }
        candidates.push(PathBuf::from(
            "/Library/Application Support/com.apple.TCC/TCC.db",
        ));

        let displayed: Vec<String> = candidates
            .iter()
            .map(|path| path.display().to_string())
            .collect();
        let outcomes = displayed.iter().zip(&candidates).map(|(shown, path)| {
            (
                shown.as_str(),
                std::fs::File::open(path).map(|_| ()).map_err(|e| e.kind()),
            )
        });

        let (state, detail) = resolve_full_disk_access(outcomes);
        PermissionProbe::new(ID_FULL_DISK_ACCESS, state, detail)
    }

    fn probe_accessibility() -> PermissionProbe {
        // SAFETY: no arguments, no ownership transfer; documented prompt-free.
        let trusted = unsafe { AXIsProcessTrusted() } != 0;
        PermissionProbe::new(
            ID_ACCESSIBILITY,
            if trusted {
                PermissionState::Granted
            } else {
                // The API cannot distinguish "denied" from "never asked".
                PermissionState::Denied
            },
            Some("AXIsProcessTrusted".to_string()),
        )
    }

    fn probe_screen_recording() -> PermissionProbe {
        // SAFETY: the preflight variant explicitly does not request access.
        let granted = unsafe { CGPreflightScreenCaptureAccess() } != 0;
        PermissionProbe::new(
            ID_SCREEN_RECORDING,
            if granted {
                PermissionState::Granted
            } else {
                PermissionState::Denied
            },
            Some("CGPreflightScreenCaptureAccess".to_string()),
        )
    }

    fn probe_input_monitoring() -> PermissionProbe {
        // SAFETY: a plain status query on a constant request type.
        let access = unsafe { IOHIDCheckAccess(K_IOHID_REQUEST_TYPE_LISTEN_EVENT) };
        PermissionProbe::new(
            ID_INPUT_MONITORING,
            classify_hid_access(access),
            Some(format!("IOHIDCheckAccess={access}")),
        )
    }

    /// Local network access, decided by actually trying to use it.
    ///
    /// There is no preflight API for this one, so a UDP datagram to the default
    /// gateway is the probe: a denied app gets `EHOSTUNREACH`, per TN3179. The
    /// first call is also what makes macOS show the prompt, so a `Denied` here
    /// can simply mean "the user has not answered yet" — the panel says so and
    /// offers a re-run.
    ///
    /// The target is the gateway and not a multicast group, even though TN3179
    /// counts multicast as local network. Measured on macOS 26 with the
    /// local-network grant active: unicast and TCP to the gateway succeed while
    /// multicast *and* broadcast still return `EHOSTUNREACH`, because those
    /// need `com.apple.developer.networking.multicast` on top — an entitlement
    /// Apple issues only on request. A multicast probe therefore reports denied
    /// forever regardless of the state the panel is asking about.
    ///
    /// The gateway rather than an arbitrary host on the subnet, because a LAN
    /// address that does not answer ARP returns `EHOSTUNREACH` as well and
    /// would be indistinguishable from a denial.
    fn probe_local_network() -> PermissionProbe {
        let Some(gateway) = default_gateway() else {
            return PermissionProbe::new(
                ID_LOCAL_NETWORK,
                PermissionState::Unknown,
                Some("no private IPv4 default gateway".to_string()),
            );
        };

        // SAFETY: every pointer below refers to a live local, and the socket is
        // closed on both paths before returning.
        unsafe {
            let fd = libc::socket(libc::AF_INET, libc::SOCK_DGRAM, 0);
            if fd < 0 {
                return PermissionProbe::new(
                    ID_LOCAL_NETWORK,
                    PermissionState::Unknown,
                    Some("socket() failed".to_string()),
                );
            }

            let mut addr: libc::sockaddr_in = std::mem::zeroed();
            addr.sin_len = std::mem::size_of::<libc::sockaddr_in>() as u8;
            addr.sin_family = libc::AF_INET as libc::sa_family_t;
            // Port 9 is discard: nothing on the network acts on the datagram.
            addr.sin_port = 9u16.to_be();
            // Written byte-for-byte because `s_addr` is already in network order.
            addr.sin_addr.s_addr = u32::from_ne_bytes(gateway);

            let payload = [0u8; 1];
            let sent = libc::sendto(
                fd,
                payload.as_ptr().cast(),
                payload.len(),
                0,
                std::ptr::addr_of!(addr).cast(),
                std::mem::size_of::<libc::sockaddr_in>() as libc::socklen_t,
            );
            let errno = *libc::__error();
            libc::close(fd);

            let (state, detail) = classify_local_network_probe(sent, errno);
            let [a, b, c, d] = gateway;
            PermissionProbe::new(
                ID_LOCAL_NETWORK,
                state,
                detail.map(|reason| format!("{reason} -> {a}.{b}.{c}.{d}")),
            )
        }
    }

    /// The IPv4 default gateway, or `None` when it is not a private address.
    fn default_gateway() -> Option<[u8; 4]> {
        let output = std::process::Command::new("/sbin/route")
            .args(["-n", "get", "default"])
            .output()
            .ok()?;
        parse_default_gateway(&String::from_utf8_lossy(&output.stdout))
    }

    /// A protected user folder, decided by listing it.
    ///
    /// Unlike the Full Disk Access probe this *does* prompt when the user has
    /// never been asked, which is why it only runs when explicitly requested.
    fn probe_folder(id: &str, relative: &str) -> PermissionProbe {
        let Some(home) = home_dir() else {
            return PermissionProbe::new(
                id,
                PermissionState::Unknown,
                Some("HOME not set".to_string()),
            );
        };
        let path = home.join(relative);
        match std::fs::read_dir(&path) {
            Ok(_) => PermissionProbe::new(id, PermissionState::Granted, Some(relative.to_string())),
            Err(error) if error.kind() == std::io::ErrorKind::PermissionDenied => {
                PermissionProbe::new(id, PermissionState::Denied, Some("EACCES".to_string()))
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                // No folder, no restriction to report.
                PermissionProbe::new(id, PermissionState::NotRequired, Some("absent".to_string()))
            }
            Err(error) => {
                PermissionProbe::new(id, PermissionState::Unknown, Some(error.to_string()))
            }
        }
    }

    pub fn probes(active: &[String], os_version: Option<&str>) -> Vec<PermissionProbe> {
        let wants = |id: &str| active.iter().any(|entry| entry == id);

        let local_network = if !os_version.is_some_and(local_network_gate_applies) {
            PermissionProbe::new(
                ID_LOCAL_NETWORK,
                PermissionState::NotRequired,
                Some("macOS < 15".to_string()),
            )
        } else if wants(ID_LOCAL_NETWORK) {
            probe_local_network()
        } else {
            PermissionProbe::new(ID_LOCAL_NETWORK, PermissionState::NotProbed, None)
        };

        let folder = |id: &'static str, relative: &'static str| {
            if wants(id) {
                probe_folder(id, relative)
            } else {
                PermissionProbe::new(id, PermissionState::NotProbed, None)
            }
        };

        vec![
            probe_full_disk_access(),
            probe_accessibility(),
            probe_screen_recording(),
            probe_input_monitoring(),
            local_network,
            folder(ID_DESKTOP_FOLDER, "Desktop"),
            folder(ID_DOCUMENTS_FOLDER, "Documents"),
            folder(ID_DOWNLOADS_FOLDER, "Downloads"),
        ]
    }

    pub fn open_pane(url: &str) -> Result<(), String> {
        std::process::Command::new("/usr/bin/open")
            .arg(url)
            .status()
            .map_err(|error| error.to_string())
            .and_then(|status| {
                if status.success() {
                    Ok(())
                } else {
                    Err(format!("open exited with {status}"))
                }
            })
    }
}

/// Gather everything the settings panel shows.
///
/// `active` names the ids the user has consented to probe by side effect;
/// anything not listed comes back as [`PermissionState::NotProbed`].
#[cfg(target_os = "macos")]
pub fn collect_report(active: &[String], bundle_id: Option<String>) -> PermissionReport {
    let os_version = imp::os_version();
    PermissionReport {
        supported: true,
        probes: imp::probes(active, os_version.as_deref()),
        signing: imp::signing_identity(),
        os_version,
        bundle_id,
    }
}

#[cfg(not(target_os = "macos"))]
pub fn collect_report(_active: &[String], bundle_id: Option<String>) -> PermissionReport {
    PermissionReport {
        supported: false,
        os_version: None,
        bundle_id,
        signing: None,
        probes: Vec::new(),
    }
}

/// Open the System Settings pane that governs `id`.
pub fn open_privacy_pane(id: &str) -> Result<(), String> {
    let url = privacy_pane_url(id).ok_or_else(|| format!("unknown privacy pane: {id}"))?;

    #[cfg(target_os = "macos")]
    {
        imp::open_pane(url)
    }

    #[cfg(not(target_os = "macos"))]
    {
        let _ = url;
        Err("privacy panes are macOS-only".to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// M-13 — `Info.plist` is the oracle, not the list beside the notice.
    ///
    /// macOS reads the plist; the merge notice reads
    /// [`TCC_GRANTS_RESET_CATEGORIES`]. If a usage description is added to the
    /// plist and not here, the notice quietly understates which approvals the
    /// user is about to have to give again — which is the one thing this
    /// unmigratable root is supposed to prevent. Reading the file rather than
    /// restating its contents is what makes that detectable.
    #[test]
    fn tcc_reset_categories_match_the_declared_usage_descriptions() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Info.plist");
        let plist = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("read {} failed: {e}", path.display()));

        let declared: std::collections::BTreeSet<String> = plist
            .lines()
            .filter_map(|line| {
                let key = line.trim().strip_prefix("<key>")?.strip_suffix("</key>")?;
                key.ends_with("UsageDescription").then(|| key.to_string())
            })
            .collect();
        assert!(
            !declared.is_empty(),
            "{} declares no NS*UsageDescription keys; this test has lost its subject",
            path.display()
        );

        let listed: std::collections::BTreeSet<String> = TCC_GRANTS_RESET_CATEGORIES
            .iter()
            .map(|key| (*key).to_string())
            .collect();
        assert_eq!(
            listed,
            declared,
            "TCC_GRANTS_RESET_CATEGORIES has drifted from {}. Every declared \
             usage description is a privacy approval the bundle-id change \
             resets, so the merge notice must name all of them and nothing else.",
            path.display()
        );
    }

    /// The notice is platform-gated, and the gate is the platform rather than a
    /// build flag: a non-macOS build must not render a macOS-only warning.
    #[test]
    fn the_tcc_notice_is_offered_exactly_on_macos() {
        assert_eq!(
            tcc_grants_reset_notice().is_some(),
            cfg!(target_os = "macos")
        );
    }

    #[test]
    fn treats_macos_15_and_newer_as_gated() {
        assert!(local_network_gate_applies("15.0"));
        assert!(local_network_gate_applies("26.1.2"));
        assert!(!local_network_gate_applies("14.7.2"));
        // An unparseable version must not silently claim the gate applies.
        assert!(!local_network_gate_applies("unknown"));
    }

    #[test]
    fn reads_ehostunreach_as_the_local_network_denial() {
        let (state, detail) = classify_local_network_probe(-1, libc::EHOSTUNREACH);
        assert_eq!(state, PermissionState::Denied);
        assert_eq!(detail.as_deref(), Some("EHOSTUNREACH"));
    }

    #[test]
    fn treats_a_successful_send_as_granted() {
        let (state, _) = classify_local_network_probe(1, 0);
        assert_eq!(state, PermissionState::Granted);
    }

    #[test]
    fn keeps_routing_failures_out_of_the_denied_bucket() {
        // No route says nothing about permission; reporting it as denied would
        // send the user to a settings pane that cannot fix anything.
        assert_eq!(
            classify_local_network_probe(-1, libc::ENETUNREACH).0,
            PermissionState::Unknown
        );
        assert_eq!(
            classify_local_network_probe(-1, libc::ETIMEDOUT).0,
            PermissionState::Unknown
        );
    }

    #[test]
    fn walks_past_a_missing_tcc_database_to_the_next_candidate() {
        // The per-user database is absent on macOS 26. Treating that as the end
        // of the walk reports Unknown on a machine whose system database gives a
        // definite answer one path further down.
        let (state, detail) = resolve_full_disk_access([
            ("~/Library/.../TCC.db", Err(std::io::ErrorKind::NotFound)),
            (
                "/Library/.../TCC.db",
                Err(std::io::ErrorKind::PermissionDenied),
            ),
        ]);
        assert_eq!(state, PermissionState::Denied);
        assert_eq!(detail.as_deref(), Some("EACCES /Library/.../TCC.db"));
    }

    #[test]
    fn reports_full_disk_access_on_the_first_readable_database() {
        let (state, _) = resolve_full_disk_access([
            ("~/Library/.../TCC.db", Ok(())),
            (
                "/Library/.../TCC.db",
                Err(std::io::ErrorKind::PermissionDenied),
            ),
        ]);
        assert_eq!(state, PermissionState::Granted);
    }

    #[test]
    fn keeps_full_disk_access_unknown_when_nothing_answers() {
        let (state, _) = resolve_full_disk_access([
            ("a", Err(std::io::ErrorKind::NotFound)),
            ("b", Err(std::io::ErrorKind::NotFound)),
        ]);
        assert_eq!(state, PermissionState::Unknown);
    }

    #[test]
    fn reads_the_ipv4_default_gateway() {
        let output = "   route to: default\ndestination: default\n       mask: default\n    \
             gateway: 192.168.5.1\n  interface: en0\n";
        assert_eq!(parse_default_gateway(output), Some([192, 168, 5, 1]));
    }

    #[test]
    fn accepts_every_private_range_a_home_router_uses() {
        for (raw, expected) in [
            ("10.0.0.1", [10, 0, 0, 1]),
            ("172.16.0.1", [172, 16, 0, 1]),
            ("172.31.255.254", [172, 31, 255, 254]),
            ("169.254.1.1", [169, 254, 1, 1]),
        ] {
            assert_eq!(
                parse_default_gateway(&format!("gateway: {raw}\n")),
                Some(expected),
                "{raw}"
            );
        }
    }

    #[test]
    fn rejects_a_tunnel_peer_posing_as_the_default_gateway() {
        // A full-tunnel VPN owns the default route, but its peer is not on the
        // local network. Probing it would answer a different question and
        // report "granted" for an app that still cannot reach the LAN.
        assert_eq!(parse_default_gateway("gateway: 198.18.0.1\n"), None);
        assert_eq!(parse_default_gateway("gateway: 100.64.0.1\n"), None);
        assert_eq!(parse_default_gateway("gateway: 8.8.8.8\n"), None);
        // 172.32 is outside 172.16/12 and must not pass as private.
        assert_eq!(parse_default_gateway("gateway: 172.32.0.1\n"), None);
        assert_eq!(parse_default_gateway("gateway: 172.15.0.1\n"), None);
    }

    #[test]
    fn rejects_a_gateway_that_is_not_an_address() {
        // Interfaces with no next hop print `link#30`.
        assert_eq!(parse_default_gateway("gateway: link#30\n"), None);
        assert_eq!(parse_default_gateway("interface: en0\n"), None);
        assert_eq!(parse_default_gateway("gateway: 192.168.5\n"), None);
        assert_eq!(parse_default_gateway("gateway: 192.168.5.1.7\n"), None);
        assert_eq!(parse_default_gateway("gateway: 192.168.5.999\n"), None);
    }

    #[test]
    fn maps_hid_access_codes() {
        assert_eq!(classify_hid_access(0), PermissionState::Granted);
        assert_eq!(classify_hid_access(1), PermissionState::Denied);
        assert_eq!(classify_hid_access(2), PermissionState::Unknown);
    }

    #[test]
    fn reads_an_adhoc_signature_as_unstable() {
        let output = "Executable=/Applications/Termul Manager.app/Contents/MacOS/Termul Manager\n\
             Identifier=com.se-manager.app\n\
             CodeDirectory v=20400 size=100 flags=0x20002(adhoc,linker-signed) hashes=3+2\n\
             Signature=adhoc\n\
             Info.plist entries=42\n\
             TeamIdentifier=not set\n";
        let identity = parse_codesign_output(output);
        assert_eq!(identity.kind, SigningKind::Adhoc);
        assert_eq!(identity.team_id, None);
        assert!(!identity.grants_survive_rebuild);
    }

    #[test]
    fn reads_a_developer_id_signature_as_stable() {
        let output = "Identifier=com.se-manager.app\n\
             Signature=adhoc-not-really\n\
             Authority=Developer ID Application: Someone (ABCDE12345)\n\
             Authority=Developer ID Certification Authority\n\
             TeamIdentifier=ABCDE12345\n";
        let identity = parse_codesign_output(output);
        assert_eq!(identity.kind, SigningKind::DeveloperId);
        assert_eq!(identity.team_id.as_deref(), Some("ABCDE12345"));
        assert!(identity.grants_survive_rebuild);
    }

    #[test]
    fn treats_team_identifier_not_set_as_absent() {
        let identity = parse_codesign_output("TeamIdentifier=not set\nAuthority=Whatever\n");
        assert_eq!(identity.team_id, None);
    }

    #[test]
    fn detects_an_unsigned_binary() {
        let identity = parse_codesign_output("/path/to/thing: code object is not signed at all\n");
        assert_eq!(identity.kind, SigningKind::Unsigned);
        assert!(!identity.grants_survive_rebuild);
    }

    #[test]
    fn resolves_a_pane_for_every_reported_probe_id() {
        // The renderer's "Open Settings" button is driven by the same ids the
        // report hands back, so a probe with no pane would render a dead button.
        for id in [
            ID_FULL_DISK_ACCESS,
            ID_ACCESSIBILITY,
            ID_SCREEN_RECORDING,
            ID_INPUT_MONITORING,
            ID_LOCAL_NETWORK,
            ID_DESKTOP_FOLDER,
            ID_DOCUMENTS_FOLDER,
            ID_DOWNLOADS_FOLDER,
        ] {
            assert!(privacy_pane_url(id).is_some(), "no pane for {id}");
        }
    }

    #[test]
    fn refuses_an_unknown_pane_id() {
        assert!(privacy_pane_url("../../etc/passwd").is_none());
        assert!(privacy_pane_url("https://example.com").is_none());
        assert!(open_privacy_pane("nope").is_err());
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn errno_constants_match_libc() {
        // The classifier hardcodes these so it builds off macOS; this is the
        // guard that a typo would otherwise hide until a user hit it.
        assert_eq!(EPERM, libc::EPERM);
        assert_eq!(EACCES, libc::EACCES);
        assert_eq!(ENETDOWN, libc::ENETDOWN);
        assert_eq!(ENETUNREACH, libc::ENETUNREACH);
        assert_eq!(EHOSTUNREACH, libc::EHOSTUNREACH);
    }

    #[test]
    fn marks_only_the_prompting_probes_as_active() {
        assert!(PermissionProbe::new(ID_LOCAL_NETWORK, PermissionState::Unknown, None).active);
        assert!(PermissionProbe::new(ID_DESKTOP_FOLDER, PermissionState::Unknown, None).active);
        // The preflight probes must stay passive or the panel would refuse to
        // run them without a click.
        assert!(!PermissionProbe::new(ID_ACCESSIBILITY, PermissionState::Unknown, None).active);
        assert!(!PermissionProbe::new(ID_FULL_DISK_ACCESS, PermissionState::Unknown, None).active);
    }
}
