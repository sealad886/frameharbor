//! FrameHarbor's evidence and source-safety contracts.
#![forbid(unsafe_code)]

pub mod acquisition;
pub mod bmff;
pub mod checkpoint;
pub mod contracts;
pub mod control;
pub mod discovery;
pub mod filesystem;
pub mod fragmented;
pub mod jobs;
pub mod ledger;
pub mod performance;
pub mod publication;
pub mod runner;
pub mod segments;
pub mod session;
mod source_archive;
pub mod storage;
mod supervision;
pub mod validation;
