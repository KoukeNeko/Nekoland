use crate::Nekoland;
use smithay::{
    input::pointer::{
        AxisFrame, ButtonEvent, GestureHoldBeginEvent, GestureHoldEndEvent, GesturePinchBeginEvent,
        GesturePinchEndEvent, GesturePinchUpdateEvent, GestureSwipeBeginEvent, GestureSwipeEndEvent,
        GestureSwipeUpdateEvent, GrabStartData as PointerGrabStartData, MotionEvent, PointerGrab,
        PointerInnerHandle, RelativeMotionEvent,
    },
    reexports::wayland_server::protocol::wl_surface::WlSurface,
    utils::{Logical, Point},
};

/// The bar between windows side by side, held by the pointer while it is dragged.
pub struct DividerGrab {
    start_data: PointerGrabStartData<Nekoland>,
}

impl DividerGrab {
    pub fn new(start_data: PointerGrabStartData<Nekoland>) -> Self {
        Self { start_data }
    }
}

impl PointerGrab<Nekoland> for DividerGrab {
    fn motion(
        &mut self,
        data: &mut Nekoland,
        handle: &mut PointerInnerHandle<'_, Nekoland>,
        _focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &MotionEvent,
    ) {
        // While the bar is held, no client has the pointer.
        handle.motion(data, None, event);
        data.drag_divider(event.location, false);
    }

    fn relative_motion(
        &mut self,
        data: &mut Nekoland,
        handle: &mut PointerInnerHandle<'_, Nekoland>,
        focus: Option<(WlSurface, Point<f64, Logical>)>,
        event: &RelativeMotionEvent,
    ) {
        handle.relative_motion(data, focus, event);
    }

    fn button(
        &mut self,
        data: &mut Nekoland,
        handle: &mut PointerInnerHandle<'_, Nekoland>,
        event: &ButtonEvent,
    ) {
        handle.button(data, event);

        // The button is a button code as defined in the
        // Linux kernel's linux/input-event-codes.h header file, e.g. BTN_LEFT.
        const BTN_LEFT: u32 = 0x110;

        if !handle.current_pressed().contains(&BTN_LEFT) {
            handle.unset_grab(self, data, event.serial, event.time, true);
        }
    }

    fn axis(
        &mut self,
        data: &mut Nekoland,
        handle: &mut PointerInnerHandle<'_, Nekoland>,
        details: AxisFrame,
    ) {
        handle.axis(data, details)
    }

    fn frame(&mut self, data: &mut Nekoland, handle: &mut PointerInnerHandle<'_, Nekoland>) {
        handle.frame(data);
    }

    fn gesture_swipe_begin(
        &mut self,
        data: &mut Nekoland,
        handle: &mut PointerInnerHandle<'_, Nekoland>,
        event: &GestureSwipeBeginEvent,
    ) {
        handle.gesture_swipe_begin(data, event)
    }

    fn gesture_swipe_update(
        &mut self,
        data: &mut Nekoland,
        handle: &mut PointerInnerHandle<'_, Nekoland>,
        event: &GestureSwipeUpdateEvent,
    ) {
        handle.gesture_swipe_update(data, event)
    }

    fn gesture_swipe_end(
        &mut self,
        data: &mut Nekoland,
        handle: &mut PointerInnerHandle<'_, Nekoland>,
        event: &GestureSwipeEndEvent,
    ) {
        handle.gesture_swipe_end(data, event)
    }

    fn gesture_pinch_begin(
        &mut self,
        data: &mut Nekoland,
        handle: &mut PointerInnerHandle<'_, Nekoland>,
        event: &GesturePinchBeginEvent,
    ) {
        handle.gesture_pinch_begin(data, event)
    }

    fn gesture_pinch_update(
        &mut self,
        data: &mut Nekoland,
        handle: &mut PointerInnerHandle<'_, Nekoland>,
        event: &GesturePinchUpdateEvent,
    ) {
        handle.gesture_pinch_update(data, event)
    }

    fn gesture_pinch_end(
        &mut self,
        data: &mut Nekoland,
        handle: &mut PointerInnerHandle<'_, Nekoland>,
        event: &GesturePinchEndEvent,
    ) {
        handle.gesture_pinch_end(data, event)
    }

    fn gesture_hold_begin(
        &mut self,
        data: &mut Nekoland,
        handle: &mut PointerInnerHandle<'_, Nekoland>,
        event: &GestureHoldBeginEvent,
    ) {
        handle.gesture_hold_begin(data, event)
    }

    fn gesture_hold_end(
        &mut self,
        data: &mut Nekoland,
        handle: &mut PointerInnerHandle<'_, Nekoland>,
        event: &GestureHoldEndEvent,
    ) {
        handle.gesture_hold_end(data, event)
    }

    fn start_data(&self) -> &PointerGrabStartData<Nekoland> {
        &self.start_data
    }

    /// However the drag ends, the windows get where the bar was let go.
    fn unset(&mut self, data: &mut Nekoland) {
        data.end_divider_drag();
    }
}
