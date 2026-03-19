use std::io::{self, Write};
use windows::{
    Win32::{
        Foundation::*,
        Media::*,
        Security::*,
        System::{Console::*, LibraryLoader::*, Performance::*, Threading::*},
    },
    core::*,
};

unsafe extern "system" fn ctrl_handler(ctrl_type: u32) -> BOOL {
    match ctrl_type {
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT | CTRL_LOGOFF_EVENT
        | CTRL_SHUTDOWN_EVENT => {
            reset_to_default();
            BOOL(1)
        }
        _ => BOOL(0),
    }
}

fn register_ctrl_handler() {
    unsafe {
        let _ = SetConsoleCtrlHandler(Some(ctrl_handler), true);
    }
}

#[allow(non_snake_case)]
type NtQueryTimerResolution = unsafe extern "system" fn(
    MinimumResolution: *mut u32,
    MaximumResolution: *mut u32,
    CurrentResolution: *mut u32,
) -> i32;

struct CleanupHandler;

struct MutexHandle(HANDLE);

impl Drop for CleanupHandler {
    fn drop(&mut self) {}
}

impl Drop for MutexHandle {
    fn drop(&mut self) {
        unsafe {
            let _ = CloseHandle(self.0);
        }
    }
}

fn stdout_handle() -> Option<HANDLE> {
    unsafe {
        match GetStdHandle(STD_OUTPUT_HANDLE) {
            Ok(h) if h != INVALID_HANDLE_VALUE => Some(h),
            _ => None,
        }
    }
}

pub fn clear_console() {
    unsafe {
        let handle = match stdout_handle() {
            Some(h) => h,
            None => return,
        };

        let mut csbi = CONSOLE_SCREEN_BUFFER_INFO::default();
        if GetConsoleScreenBufferInfo(handle, &mut csbi).is_err() {
            return;
        }

        let cell_count = (csbi.dwSize.X as u32) * (csbi.dwSize.Y as u32);
        let origin = COORD { X: 0, Y: 0 };
        let mut written = 0u32;

        let _ = FillConsoleOutputCharacterW(handle, ' ' as u16, cell_count, origin, &mut written);
        let _ = FillConsoleOutputAttribute(
            handle,
            csbi.wAttributes.0,
            cell_count,
            origin,
            &mut written,
        );
        let _ = SetConsoleCursorPosition(handle, origin);

        let cursor_info = CONSOLE_CURSOR_INFO {
            dwSize: 1,
            bVisible: BOOL(0),
        };
        let _ = SetConsoleCursorInfo(handle, &cursor_info);
    }
}

fn is_running_as_admin() -> bool {
    unsafe {
        let mut token_handle = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token_handle).is_err() {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = size_of::<TOKEN_ELEVATION>() as u32;

        let result = GetTokenInformation(
            token_handle,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            size,
            &mut size,
        )
        .is_ok()
            && elevation.TokenIsElevated != 0;

        let _ = CloseHandle(token_handle);
        result
    }
}

fn create_app_mutex() -> Option<MutexHandle> {
    unsafe {
        match CreateMutexW(None, false, w!("Global\\InputLagApplicationMutex")) {
            Ok(handle) if GetLastError() != ERROR_ALREADY_EXISTS => Some(MutexHandle(handle)),
            _ => None,
        }
    }
}

fn reset_to_default() -> bool {
    unsafe {
        let _ = timeEndPeriod(1);
        if timeBeginPeriod(16) == TIMERR_NOERROR {
            println!("Successfully reset to default (~15.6ms)");
            true
        } else {
            println!("Failed to reset to default.");
            false
        }
    }
}

fn get_caps() -> Option<(u32, u32)> {
    unsafe {
        let mut caps = TIMECAPS::default();
        if timeGetDevCaps(&mut caps, size_of::<TIMECAPS>() as u32) == TIMERR_NOERROR {
            Some((caps.wPeriodMin, caps.wPeriodMax))
        } else {
            None
        }
    }
}

fn set_custom() -> bool {
    let (min, max) = match get_caps() {
        Some(caps) => caps,
        None => {
            println!("Failed to query timer capabilities.");
            return false;
        }
    };

    let target: u32 = 1;
    let period = target.clamp(min, max);

    unsafe {
        timeBeginPeriod(period);
    }

    if period != target {
        println!(
            "Requested {}ms is out of range [{min}ms, {max}ms]. Set to {period}ms instead.",
            target
        );
    } else {
        println!("Timer resolution set to: {period}ms");
    }

    true
}

fn measure(iterations: u32) {
    unsafe {
        let h_ntdll = match LoadLibraryW(w!("NtDll.dll")) {
            Ok(h) => h,
            Err(_) => {
                println!("Failed to load NtDll.dll");
                return;
            }
        };

        let nt_query_timer_resolution: NtQueryTimerResolution =
            match GetProcAddress(h_ntdll, s!("NtQueryTimerResolution")) {
                Some(addr) => std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    NtQueryTimerResolution,
                >(addr),
                None => {
                    println!("Failed to resolve NtQueryTimerResolution.");
                    let _ = FreeLibrary(h_ntdll);
                    return;
                }
            };

        let mut freq = 0i64;
        if QueryPerformanceFrequency(&mut freq).is_err() || freq == 0 {
            println!("QueryPerformanceFrequency failed.");
            let _ = FreeLibrary(h_ntdll);
            return;
        }

        let mut total_elapsed = 0.0f64;

        for _ in 0..iterations {
            let mut start = 0i64;
            let mut end = 0i64;
            let _ = QueryPerformanceCounter(&mut start);
            Sleep(1);
            let _ = QueryPerformanceCounter(&mut end);
            total_elapsed += (end - start) as f64 / freq as f64 * 1000.0;
        }

        let avg_elapsed = total_elapsed / iterations as f64;

        let mut min_res = 0u32;
        let mut max_res = 0u32;
        let mut cur_res = 0u32;
        nt_query_timer_resolution(&mut min_res, &mut max_res, &mut cur_res);

        println!(
            "Average over {iterations} iterations: {avg_elapsed:.3}ms \
             (Resolution: {:.3}ms, Min: {:.3}ms, Max: {:.3}ms)",
            cur_res as f64 / 10_000.0,
            min_res as f64 / 10_000.0,
            max_res as f64 / 10_000.0,
        );

        let _ = FreeLibrary(h_ntdll);
    }
}

fn pause() {
    println!("Press Enter to continue...");
    let mut buf = String::new();
    let _ = io::stdin().read_line(&mut buf);
}

fn main() {
    let _cleanup = CleanupHandler;
    register_ctrl_handler();

    let _mutex = match create_app_mutex() {
        Some(h) => h,
        None => {
            println!("Another instance is already running. Please close it first.");
            pause();
            return;
        }
    };

    if !is_running_as_admin() {
        println!("Administrator privileges are required. Please restart as administrator.");
        pause();
        return;
    }

    loop {
        clear_console();
        println!("1. Set timer resolution to 1ms");
        println!("2. Measure sleep accuracy (100 iterations)");
        println!("3. Exit");
        println!("4. Reset to default (~15.6ms)");
        print!("\nSelect an option (1-4): ");
        let _ = io::stdout().flush();

        let mut input = String::new();
        if io::stdin().read_line(&mut input).is_err() {
            break;
        }

        match input.trim() {
            "1" => {
                set_custom();
                pause();
            }
            "2" => {
                measure(100);
                pause();
            }
            "3" => {
                println!("Exiting...");
                break;
            }
            "4" => {
                reset_to_default();
                pause();
            }
            _ => {
                println!("Invalid option. Please enter 1, 2, 3, or 4.");
                pause();
            }
        }
    }
}
