use std::sync::OnceLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum LogLevel {
    Error,
    Warning,
    Info,
    Debug,
}

static LOG_LEVEL: OnceLock<LogLevel> = OnceLock::new();

pub fn init_logging(verbose: bool) {
    let level = if verbose {
        LogLevel::Debug
    } else {
        LogLevel::Info
    };
    LOG_LEVEL.set(level).ok(); // Ignore errors if already set
}

pub fn get_log_level() -> LogLevel {
    *LOG_LEVEL.get().unwrap_or(&LogLevel::Info)
}

pub fn log(level: LogLevel, message: &str) {
    if level <= get_log_level() {
        match level {
            LogLevel::Error => eprintln!("Error: {}", message),
            LogLevel::Warning => eprintln!("Warning: {}", message),
            LogLevel::Info => println!("{}", message),
            LogLevel::Debug => println!("Debug: {}", message),
        }
    }
}

#[macro_export]
macro_rules! log_error {
    ($($arg:tt)*) => {
        $crate::logging::log($crate::logging::LogLevel::Error, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_warning {
    ($($arg:tt)*) => {
        $crate::logging::log($crate::logging::LogLevel::Warning, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_info {
    ($($arg:tt)*) => {
        $crate::logging::log($crate::logging::LogLevel::Info, &format!($($arg)*))
    };
}

#[macro_export]
macro_rules! log_debug {
    ($($arg:tt)*) => {
        $crate::logging::log($crate::logging::LogLevel::Debug, &format!($($arg)*))
    };
}