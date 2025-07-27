#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

// Declare all modules as public so they can be used by the binary and tests.
pub mod app;
pub mod config;
pub mod core;
pub mod utils;
