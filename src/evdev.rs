//! Keyboards and mice read straight from `/dev/input`, for a system without udev. libinput
//! takes only the devices udev's database calls input, and unkernel's guest (Linux built to
//! WebAssembly, https://github.com/pithings/unkernel) has no udev: its keyboard, mouse and
//! tablet would otherwise do nothing.
//!
//! shortcut: the devices there at start, none plugged in later; no pointer acceleration, touch
//! or tablet tools, which unkernel's devices do not have.

use std::{
    fs::File,
    io::{self, Read},
    os::{fd::AsRawFd, unix::fs::OpenOptionsExt},
    path::{Path, PathBuf},
};

use smithay::{
    backend::input::{
        AbsolutePositionEvent, Axis, AxisRelativeDirection, AxisSource, ButtonState, Device,
        DeviceCapability, Event, InputBackend, InputEvent, KeyState, KeyboardKeyEvent, Keycode,
        PointerAxisEvent, PointerButtonEvent, PointerMotionAbsoluteEvent, PointerMotionEvent, UnusedEvent,
    },
    reexports::calloop::{generic::Generic, Interest, LoopHandle, Mode, PostAction},
};
use tracing::{info, warn};

use crate::Nekoland;

const INPUT_DIRECTORY: &str = "/dev/input";
const EVENT_DEVICE_PREFIX: &str = "event";

// linux/input-event-codes.h
const EV_SYN: u16 = 0x00;
const EV_KEY: u16 = 0x01;
const EV_REL: u16 = 0x02;
const EV_ABS: u16 = 0x03;
const SYN_REPORT: u16 = 0;
const REL_X: u16 = 0x00;
const REL_Y: u16 = 0x01;
const REL_HWHEEL: u16 = 0x06;
const REL_WHEEL: u16 = 0x08;
const ABS_X: u16 = 0x00;
const ABS_Y: u16 = 0x01;
/// The mouse's buttons, BTN_LEFT to BTN_TASK; every other key code is a keyboard's.
const MOUSE_BUTTONS: std::ops::RangeInclusive<u16> = 0x110..=0x117;
const KEY_RELEASED: i32 = 0;
const KEY_PRESSED: i32 = 1;

/// evdev's key codes are XKB's less 8, X's first eight codes having been reserved.
const XKB_KEYCODE_OFFSET: u32 = 8;
/// How far one notch of a wheel scrolls, in libinput's degrees, and in high-resolution units.
const WHEEL_NOTCH_DEGREES: f64 = 15.0;
const WHEEL_NOTCH_V120: f64 = 120.0;
const MICROSECONDS_PER_SECOND: u64 = 1_000_000;
/// The events taken from a device in one read.
const EVENTS_PER_READ: usize = 64;
const DEVICE_NAME_LENGTH: usize = 256;

// linux/input.h: the ioctls asking a device what it is, all reads of type 'E'.
const IOC_READ: u64 = 2;
const IOC_DIRECTION_SHIFT: u64 = 30;
const IOC_SIZE_SHIFT: u64 = 16;
const IOC_TYPE_SHIFT: u64 = 8;
const EVDEV_IOCTL_TYPE: u64 = b'E' as u64;
const EVIOCGNAME: u64 = 0x06;
const EVIOCGBIT: u64 = 0x20;
const EVIOCGABS: u64 = 0x40;

/// Starts reading every event device in `/dev/input`. Fails only when none could be opened.
pub fn watch(handle: &LoopHandle<'static, Nekoland>) -> io::Result<()> {
    let mut watched = 0;
    for path in event_device_paths()? {
        match open_device(&path) {
            Ok((file, reader)) => {
                info!(
                    "reading {} ({}) without libinput",
                    reader.device.name,
                    path.display()
                );
                watch_device(handle, file, reader)?;
                watched += 1;
            }
            Err(err) => warn!("could not read {}: {err}", path.display()),
        }
    }
    if watched == 0 {
        return Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no input device could be opened",
        ));
    }
    Ok(())
}

fn event_device_paths() -> io::Result<Vec<PathBuf>> {
    let mut paths: Vec<PathBuf> = std::fs::read_dir(INPUT_DIRECTORY)?
        .filter_map(Result::ok)
        .filter(|entry| {
            entry
                .file_name()
                .to_string_lossy()
                .starts_with(EVENT_DEVICE_PREFIX)
        })
        .map(|entry| entry.path())
        .collect();
    paths.sort();
    Ok(paths)
}

fn open_device(path: &Path) -> io::Result<(File, DeviceReader)> {
    let file = File::options()
        .read(true)
        .custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC | libc::O_NOCTTY)
        .open(path)?;
    let event_types = query_bits(&file, 0)?;
    let device = EvdevDevice {
        path: path.to_path_buf(),
        name: query_name(&file).unwrap_or_else(|_| path.display().to_string()),
        keyboard: has_bit(&event_types, EV_KEY),
        pointer: has_bit(&event_types, EV_REL) || has_bit(&event_types, EV_ABS),
    };
    let ranges = has_bit(&event_types, EV_ABS)
        .then(|| {
            Some((
                query_abs_range(&file, ABS_X).ok()?,
                query_abs_range(&file, ABS_Y).ok()?,
            ))
        })
        .flatten();
    Ok((file, DeviceReader::new(device, ranges)))
}

fn watch_device(
    handle: &LoopHandle<'static, Nekoland>,
    file: File,
    mut reader: DeviceReader,
) -> io::Result<()> {
    let source = Generic::new(file, Interest::READ, Mode::Level);
    handle
        .insert_source(source, move |_, file, data| {
            for event in read_events(file)? {
                reader.translate(event, |input| data.process_input_event(input));
            }
            Ok(PostAction::Continue)
        })
        .map_err(|err| err.error)?;
    Ok(())
}

/// The events waiting on a device, none once it has nothing more to give.
fn read_events(file: &File) -> io::Result<Vec<libc::input_event>> {
    let event_size = std::mem::size_of::<libc::input_event>();
    let mut bytes = vec![0u8; event_size * EVENTS_PER_READ];
    let read = match (&*file).read(&mut bytes) {
        Ok(read) => read,
        Err(err) if err.kind() == io::ErrorKind::WouldBlock => 0,
        Err(err) => return Err(err),
    };
    Ok(bytes[..read - read % event_size]
        .chunks_exact(event_size)
        // Safety: input_event is plain integers, and each chunk is one whole event the kernel wrote.
        .map(|chunk| unsafe { std::ptr::read_unaligned(chunk.as_ptr().cast::<libc::input_event>()) })
        .collect())
}

fn evdev_read_ioctl(number: u64, size: usize) -> u64 {
    (IOC_READ << IOC_DIRECTION_SHIFT)
        | ((size as u64) << IOC_SIZE_SHIFT)
        | (EVDEV_IOCTL_TYPE << IOC_TYPE_SHIFT)
        | number
}

/// Fills `buffer` through a read ioctl of the device's.
fn query_into<T>(file: &File, number: u64, buffer: &mut T) -> io::Result<()> {
    let request = evdev_read_ioctl(number, std::mem::size_of::<T>());
    // Safety: the request's size is the buffer's, which the kernel writes at most.
    let result = unsafe { libc::ioctl(file.as_raw_fd(), request as _, std::ptr::from_mut(buffer)) };
    if result < 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

/// The codes the device sends of one event type, or with 0 the event types it sends, as bits.
fn query_bits(file: &File, event_type: u16) -> io::Result<[u8; 96]> {
    let mut bits = [0u8; 96];
    query_into(file, EVIOCGBIT + u64::from(event_type), &mut bits)?;
    Ok(bits)
}

fn has_bit(bits: &[u8], bit: u16) -> bool {
    bits.get(usize::from(bit / 8))
        .is_some_and(|byte| byte & (1 << (bit % 8)) != 0)
}

fn query_name(file: &File) -> io::Result<String> {
    let mut name = [0u8; DEVICE_NAME_LENGTH];
    query_into(file, EVIOCGNAME, &mut name)?;
    let length = name.iter().position(|&byte| byte == 0).unwrap_or(name.len());
    Ok(String::from_utf8_lossy(&name[..length]).into_owned())
}

fn query_abs_range(file: &File, axis: u16) -> io::Result<AbsRange> {
    // Safety: input_absinfo is plain integers, for which zero is a value.
    let mut info: libc::input_absinfo = unsafe { std::mem::zeroed() };
    query_into(file, EVIOCGABS + u64::from(axis), &mut info)?;
    Ok(AbsRange {
        minimum: info.minimum,
        maximum: info.maximum,
    })
}

#[derive(Clone, Copy, Debug)]
struct AbsRange {
    minimum: i32,
    maximum: i32,
}

impl AbsRange {
    /// Where `value` lies across `length`, the range's ends at its edges.
    fn scale(self, value: i32, length: i32) -> f64 {
        let span = f64::from(self.maximum - self.minimum).max(1.0);
        f64::from(value - self.minimum) / span * f64::from(length)
    }
}

/// Turns one device's events into Smithay's: a report's motion and scrolling are gathered
/// until its SYN_REPORT, as libinput does, keys and buttons go at once.
struct DeviceReader {
    device: EvdevDevice,
    ranges: Option<(AbsRange, AbsRange)>,
    relative: (f64, f64),
    scroll: (f64, f64),
    position: (i32, i32),
    moved_to: bool,
}

impl DeviceReader {
    fn new(device: EvdevDevice, ranges: Option<(AbsRange, AbsRange)>) -> Self {
        Self {
            device,
            ranges,
            relative: (0.0, 0.0),
            scroll: (0.0, 0.0),
            position: (0, 0),
            moved_to: false,
        }
    }

    fn translate(&mut self, event: libc::input_event, mut emit: impl FnMut(InputEvent<EvdevInput>)) {
        let time = event_time(&event);
        match (event.type_, event.code) {
            (EV_KEY, code) => {
                if let Some(input) = self.key(code, event.value, time) {
                    emit(input);
                }
            }
            (EV_REL, REL_X) => self.relative.0 += f64::from(event.value),
            (EV_REL, REL_Y) => self.relative.1 += f64::from(event.value),
            // A wheel turned up or right is positive to evdev, and scrolls up or left to Wayland.
            (EV_REL, REL_WHEEL) => self.scroll.1 -= f64::from(event.value),
            (EV_REL, REL_HWHEEL) => self.scroll.0 += f64::from(event.value),
            (EV_ABS, ABS_X) => (self.position.0, self.moved_to) = (event.value, true),
            (EV_ABS, ABS_Y) => (self.position.1, self.moved_to) = (event.value, true),
            (EV_SYN, SYN_REPORT) => self.flush(time, &mut emit),
            _ => {}
        }
    }

    fn key(&self, code: u16, value: i32, time: u64) -> Option<InputEvent<EvdevInput>> {
        // A held key's repeats are the compositor's own.
        let pressed = match value {
            KEY_PRESSED => true,
            KEY_RELEASED => false,
            _ => return None,
        };
        let device = self.device.clone();
        Some(if MOUSE_BUTTONS.contains(&code) {
            let state = if pressed {
                ButtonState::Pressed
            } else {
                ButtonState::Released
            };
            InputEvent::PointerButton {
                event: EvdevButton {
                    device,
                    time,
                    code: u32::from(code),
                    state,
                },
            }
        } else {
            let state = if pressed {
                KeyState::Pressed
            } else {
                KeyState::Released
            };
            InputEvent::Keyboard {
                event: EvdevKey {
                    device,
                    time,
                    code: u32::from(code),
                    state,
                },
            }
        })
    }

    fn flush(&mut self, time: u64, emit: &mut impl FnMut(InputEvent<EvdevInput>)) {
        let device = &self.device;
        if self.relative != (0.0, 0.0) {
            let (delta_x, delta_y) = std::mem::take(&mut self.relative);
            emit(InputEvent::PointerMotion {
                event: EvdevMotion {
                    device: device.clone(),
                    time,
                    delta_x,
                    delta_y,
                },
            });
        }
        if std::mem::take(&mut self.moved_to) {
            if let Some(ranges) = self.ranges {
                let event = EvdevPosition {
                    device: device.clone(),
                    time,
                    ranges,
                    position: self.position,
                };
                emit(InputEvent::PointerMotionAbsolute { event });
            }
        }
        if self.scroll != (0.0, 0.0) {
            let notches = std::mem::take(&mut self.scroll);
            emit(InputEvent::PointerAxis {
                event: EvdevScroll {
                    device: device.clone(),
                    time,
                    notches,
                },
            });
        }
    }
}

fn event_time(event: &libc::input_event) -> u64 {
    let seconds = u64::try_from(event.time.tv_sec).unwrap_or_default();
    let microseconds = u64::try_from(event.time.tv_usec).unwrap_or_default();
    seconds * MICROSECONDS_PER_SECOND + microseconds
}

#[derive(Debug)]
pub struct EvdevInput;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct EvdevDevice {
    path: PathBuf,
    name: String,
    keyboard: bool,
    pointer: bool,
}

impl Device for EvdevDevice {
    fn id(&self) -> String {
        self.path.display().to_string()
    }

    fn name(&self) -> String {
        self.name.clone()
    }

    fn has_capability(&self, capability: DeviceCapability) -> bool {
        match capability {
            DeviceCapability::Keyboard => self.keyboard,
            DeviceCapability::Pointer => self.pointer,
            _ => false,
        }
    }

    fn usb_id(&self) -> Option<(u32, u32)> {
        None
    }

    fn syspath(&self) -> Option<PathBuf> {
        None
    }
}

/// The parts every event has.
macro_rules! evdev_event {
    ($name:ident { $($field:ident: $type:ty),* }) => {
        #[derive(Debug)]
        pub struct $name {
            device: EvdevDevice,
            time: u64,
            $($field: $type),*
        }

        impl Event<EvdevInput> for $name {
            fn time(&self) -> u64 {
                self.time
            }

            fn device(&self) -> EvdevDevice {
                self.device.clone()
            }
        }
    };
}

evdev_event!(EvdevKey {
    code: u32,
    state: KeyState
});
evdev_event!(EvdevButton {
    code: u32,
    state: ButtonState
});
evdev_event!(EvdevMotion {
    delta_x: f64,
    delta_y: f64
});
evdev_event!(EvdevPosition {
    ranges: (AbsRange, AbsRange),
    position: (i32, i32)
});
evdev_event!(EvdevScroll { notches: (f64, f64) });

impl KeyboardKeyEvent<EvdevInput> for EvdevKey {
    fn key_code(&self) -> Keycode {
        Keycode::new(self.code + XKB_KEYCODE_OFFSET)
    }

    fn state(&self) -> KeyState {
        self.state
    }

    fn count(&self) -> u32 {
        u32::from(self.state == KeyState::Pressed)
    }
}

impl PointerButtonEvent<EvdevInput> for EvdevButton {
    fn button_code(&self) -> u32 {
        self.code
    }

    fn state(&self) -> ButtonState {
        self.state
    }
}

impl PointerMotionEvent<EvdevInput> for EvdevMotion {
    fn delta_x(&self) -> f64 {
        self.delta_x
    }

    fn delta_y(&self) -> f64 {
        self.delta_y
    }

    fn delta_x_unaccel(&self) -> f64 {
        self.delta_x
    }

    fn delta_y_unaccel(&self) -> f64 {
        self.delta_y
    }
}

impl AbsolutePositionEvent<EvdevInput> for EvdevPosition {
    fn x(&self) -> f64 {
        f64::from(self.position.0)
    }

    fn y(&self) -> f64 {
        f64::from(self.position.1)
    }

    fn x_transformed(&self, width: i32) -> f64 {
        self.ranges.0.scale(self.position.0, width)
    }

    fn y_transformed(&self, height: i32) -> f64 {
        self.ranges.1.scale(self.position.1, height)
    }
}

impl PointerMotionAbsoluteEvent<EvdevInput> for EvdevPosition {}

impl EvdevScroll {
    fn notches(&self, axis: Axis) -> f64 {
        match axis {
            Axis::Horizontal => self.notches.0,
            Axis::Vertical => self.notches.1,
        }
    }
}

impl PointerAxisEvent<EvdevInput> for EvdevScroll {
    fn amount(&self, axis: Axis) -> Option<f64> {
        Some(self.notches(axis) * WHEEL_NOTCH_DEGREES)
    }

    fn amount_v120(&self, axis: Axis) -> Option<f64> {
        Some(self.notches(axis) * WHEEL_NOTCH_V120)
    }

    fn source(&self) -> AxisSource {
        AxisSource::Wheel
    }

    fn relative_direction(&self, _axis: Axis) -> AxisRelativeDirection {
        AxisRelativeDirection::Identical
    }
}

impl InputBackend for EvdevInput {
    type Device = EvdevDevice;
    type KeyboardKeyEvent = EvdevKey;
    type PointerAxisEvent = EvdevScroll;
    type PointerButtonEvent = EvdevButton;
    type PointerMotionEvent = EvdevMotion;
    type PointerMotionAbsoluteEvent = EvdevPosition;
    type GestureSwipeBeginEvent = UnusedEvent;
    type GestureSwipeUpdateEvent = UnusedEvent;
    type GestureSwipeEndEvent = UnusedEvent;
    type GesturePinchBeginEvent = UnusedEvent;
    type GesturePinchUpdateEvent = UnusedEvent;
    type GesturePinchEndEvent = UnusedEvent;
    type GestureHoldBeginEvent = UnusedEvent;
    type GestureHoldEndEvent = UnusedEvent;
    type TouchDownEvent = UnusedEvent;
    type TouchUpEvent = UnusedEvent;
    type TouchMotionEvent = UnusedEvent;
    type TouchCancelEvent = UnusedEvent;
    type TouchFrameEvent = UnusedEvent;
    type TabletToolAxisEvent = UnusedEvent;
    type TabletToolProximityEvent = UnusedEvent;
    type TabletToolTipEvent = UnusedEvent;
    type TabletToolButtonEvent = UnusedEvent;
    type SwitchToggleEvent = UnusedEvent;
    type SpecialEvent = UnusedEvent;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reader(ranges: Option<(AbsRange, AbsRange)>) -> DeviceReader {
        let device = EvdevDevice {
            path: PathBuf::from("/dev/input/event0"),
            name: "test".into(),
            keyboard: true,
            pointer: true,
        };
        DeviceReader::new(device, ranges)
    }

    fn raw_event(type_: u16, code: u16, value: i32) -> libc::input_event {
        // Safety: input_event is plain integers, for which zero is a value.
        let mut event: libc::input_event = unsafe { std::mem::zeroed() };
        (event.type_, event.code, event.value) = (type_, code, value);
        event
    }

    fn translate_all(reader: &mut DeviceReader, events: &[(u16, u16, i32)]) -> Vec<InputEvent<EvdevInput>> {
        let mut translated = Vec::new();
        for &(type_, code, value) in events {
            reader.translate(raw_event(type_, code, value), |input| translated.push(input));
        }
        translated
    }

    #[test]
    fn keys_take_xkb_codes_and_skip_repeats() {
        const KEY_A: u16 = 30;
        const KEY_REPEATED: i32 = 2;
        let events = translate_all(
            &mut reader(None),
            &[(EV_KEY, KEY_A, KEY_PRESSED), (EV_KEY, KEY_A, KEY_REPEATED)],
        );
        assert_eq!(events.len(), 1);
        let InputEvent::Keyboard { event } = &events[0] else {
            panic!("not a key: {:?}", events[0])
        };
        assert_eq!(event.key_code().raw(), u32::from(KEY_A) + XKB_KEYCODE_OFFSET);
        assert_eq!(event.state(), KeyState::Pressed);
    }

    #[test]
    fn mouse_buttons_are_pointer_buttons() {
        const BTN_LEFT: u16 = 0x110;
        let events = translate_all(&mut reader(None), &[(EV_KEY, BTN_LEFT, KEY_PRESSED)]);
        assert!(
            matches!(&events[..], [InputEvent::PointerButton { event }] if event.button_code() == u32::from(BTN_LEFT))
        );
    }

    #[test]
    fn motion_waits_for_the_report() {
        let mut reader = reader(None);
        assert!(translate_all(&mut reader, &[(EV_REL, REL_X, 3), (EV_REL, REL_Y, -2)]).is_empty());
        let events = translate_all(&mut reader, &[(EV_SYN, SYN_REPORT, 0)]);
        let [InputEvent::PointerMotion { event }] = &events[..] else {
            panic!("{events:?}")
        };
        assert_eq!((event.delta_x(), event.delta_y()), (3.0, -2.0));
    }

    #[test]
    fn a_wheel_turned_up_scrolls_up() {
        let events = translate_all(
            &mut reader(None),
            &[(EV_REL, REL_WHEEL, 1), (EV_SYN, SYN_REPORT, 0)],
        );
        let [InputEvent::PointerAxis { event }] = &events[..] else {
            panic!("{events:?}")
        };
        assert_eq!(event.amount(Axis::Vertical), Some(-WHEEL_NOTCH_DEGREES));
        assert_eq!(event.amount_v120(Axis::Vertical), Some(-WHEEL_NOTCH_V120));
    }

    #[test]
    fn a_tablet_spans_the_output() {
        const TABLET_MAXIMUM: i32 = 32767;
        let range = AbsRange {
            minimum: 0,
            maximum: TABLET_MAXIMUM,
        };
        let events = translate_all(
            &mut reader(Some((range, range))),
            &[
                (EV_ABS, ABS_X, TABLET_MAXIMUM),
                (EV_ABS, ABS_Y, 0),
                (EV_SYN, SYN_REPORT, 0),
            ],
        );
        let [InputEvent::PointerMotionAbsolute { event }] = &events[..] else {
            panic!("{events:?}")
        };
        assert_eq!(
            (event.x_transformed(1280), event.y_transformed(720)),
            (1280.0, 0.0)
        );
    }

    #[test]
    fn ioctl_numbers_match_linux() {
        // EVIOCGNAME(256) and EVIOCGABS(ABS_X) as linux/input.h makes them.
        assert_eq!(evdev_read_ioctl(EVIOCGNAME, 256), 0x8100_4506);
        assert_eq!(
            evdev_read_ioctl(EVIOCGABS, std::mem::size_of::<libc::input_absinfo>()),
            0x8018_4540
        );
    }
}
