use std::path::PathBuf;

/// Structure for frame path and number
#[derive(Debug)]
pub struct FrameInfo {
    pub path: PathBuf,
    pub number: u64,
}

/// Structure for second information and its frames
#[derive(Debug)]
pub struct SecondInfo {
    pub number: u64,
    pub frames: Vec<FrameInfo>,
}
