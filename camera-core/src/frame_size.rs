use std::fmt;

/// The pixel dimensions of one captured live view **frame size**.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FrameSize {
    width: u32,
    height: u32,
}

impl FrameSize {
    #[must_use]
    pub const fn new(width: u32, height: u32) -> Self {
        Self { width, height }
    }

    #[must_use]
    pub const fn width(self) -> u32 {
        self.width
    }

    #[must_use]
    pub const fn height(self) -> u32 {
        self.height
    }

    /// The area in pixels. Used to order frame sizes by size rather than by the
    /// lexicographic order of `(width, height)`.
    #[must_use]
    pub const fn area(self) -> u64 {
        (self.width as u64) * (self.height as u64)
    }
}

impl fmt::Display for FrameSize {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}x{}", self.width, self.height)
    }
}
