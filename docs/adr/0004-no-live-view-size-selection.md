# Select no live view size; measure the frame size instead

`docs/internal/plan/plan.md` §2.3.1 said the software "reads the camera's live-view size options", "selects the largest option that the camera accepts", and reads "supported frame rates and pixel formats". **That step is deleted.** Nothing in this repository selects, negotiates or reads a live view size, and nothing may be re-implemented to do so.

## Evidence

This is a pre-implementation note, written before the capture layer was built, from `docs/internal/plan/test-results.md`:

- **§4.4 step 3.** Setting `liveviewsize` to `Large` was accepted by the M50 and changed the frame size not at all. The property exists for Nikon, Fuji and Sony; on Canon it accepts the write and is a no-op.
- **§4.4 step 4.** The full frame size appeared only when the viewfinder action was pressed (`/main/actions/viewfinder=1`) in the same command as the capture, together with `output = TFT + PC`.
- **§4.5.** The frame size follows the `output` setting, so it is a consequence of a display setting and an action, not of a size request.
- **§4.4 and §4.6.** The sizes actually produced were `512x288` and `1024x576`. The camera accepted no request for either.

So there is no live view size to read and no capability to query. A program that reported a size read from a property would report a confidently wrong answer, because the property it read would not be describing the frames it is capturing.

## The replacement

An opening **calibration window** establishes the **expected frame size** from the frames themselves: the largest frame size that recurs at least a threshold number of times inside the window. A fixed count of discarded frames is rejected, because the frames to be discarded are exactly the reduced size. A window that establishes no recurring size fails loudly.

The frame size of each frame is read from the JPEG start-of-frame marker, and the frame rate from the clock across the sampling window. `1024x576` at roughly 24 frames per second appears only as a **sanity check**, never as the source of either value.

## Consequences

- `docs/internal/plan/plan.md` §2.3.1's capability probe, its "select the largest supported resolution" fallback, and its reported "actual negotiated resolution" are all void. A **requested size** remains a preference that this camera cannot honour.
- No user-facing setting asks for a frame size. The `measure` report states the **measured frame size** and the **measured frame rate**, which are the only honest answers available.
- `liveviewsize` is absent from the `PtpSession` trait, so it cannot be reached by accident. `OutputSetting` guards the one property that does change the frame size.
