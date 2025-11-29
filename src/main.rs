use crossterm::{
    cursor::{Hide, MoveTo},
    terminal::{Clear, ClearType},
    QueueableCommand,
};
use std::io::{self, stdout, Write};
use windows::{
    core::*,
    Win32::{
        Foundation::*,
        Media::*,
        Security::*,
        System::{LibraryLoader::*, Performance::*, Threading::*},
    },
};

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

pub fn clear_console() {
    let mut out = stdout();
    let _ = out.queue(Hide);
    let _ = out.queue(Clear(ClearType::All));
    let _ = out.queue(MoveTo(0, 0));
    let _ = out.flush();
}

fn is_running_as_admin() -> bool {
    unsafe {
        let mut token_handle = HANDLE::default();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token_handle).is_err() {
            return false;
        }

        let mut elevation = TOKEN_ELEVATION::default();
        let mut size = size_of::<TOKEN_ELEVATION>() as u32;

        let result = if GetTokenInformation(
            token_handle,
            TokenElevation,
            Some(&mut elevation as *mut _ as *mut _),
            size,
            &mut size,
        )
            .is_ok()
        {
            elevation.TokenIsElevated != 0
        } else {
            false
        };

        let _ = CloseHandle(token_handle);

        result
    }
}

fn create_app_mutex() -> Option<MutexHandle> {
    let mutex_name = w!("Global\\InputLagApplicationMutex");

    unsafe {
        match CreateMutexW(None, false, mutex_name) {
            Ok(handle) => {
                let last_error = GetLastError();
                if last_error == ERROR_ALREADY_EXISTS {
                    None
                } else {
                    Some(MutexHandle(handle))
                }
            }
            Err(_) => None,
        }
    }
}

fn reset_to_default() -> bool {
    unsafe {
        let _ = timeEndPeriod(1);
        let default_period = 16;
        if timeBeginPeriod(default_period) == TIMERR_NOERROR {
            println!("Successfully reset to default (~15.6ms)");
            true
        } else {
            println!("Failed to reset to default");
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
    if let Some((min, max)) = get_caps() {
        let target = 1;
        unsafe {
            if target < min {
                println!("System minimum is higher than 1ms ({}ms)", min);
                println!("Setting to system minimum: {}ms", min);
                timeBeginPeriod(min);
            } else if target > max {
                println!("1ms exceeds system maximum ({}ms)", max);
                println!("Setting to system maximum: {}ms", max);
                timeBeginPeriod(max);
            } else {
                timeBeginPeriod(target);
                println!("SC set to: {}ms", target);
            }
            return true;
        }
    }
    println!("Failed to set.");
    false
}

fn measure(iterations: u32) {
    unsafe {
        let h_ntdll = match LoadLibraryW(w!("NtDll.dll")) {
            Ok(handle) => handle,
            Err(_) => {
                println!("LoadLibrary failed");
                return;
            }
        };

        let func_name = s!("NtQueryTimerResolution");
        let nt_query_timer_resolution = match GetProcAddress(h_ntdll, func_name) {
            Some(addr) => std::mem::transmute::<_, NtQueryTimerResolution>(addr),
            None => {
                println!("Failed to load NtQueryTimerResolution");
                let _ = FreeLibrary(h_ntdll);
                return;
            }
        };

        let mut freq = 0i64;
        if QueryPerformanceFrequency(&mut freq).is_err() {
            println!("QueryPerformanceFrequency failed");
            let _ = FreeLibrary(h_ntdll);
            return;
        }

        let mut total_elapsed = 0.0;

        for _ in 0..iterations {
            let mut min_res: u32 = 0;
            let mut max_res: u32 = 0;
            let mut cur_res: u32 = 0;

            if nt_query_timer_resolution(&mut min_res, &mut max_res, &mut cur_res) != 0 {
                println!("NtQueryTimerResolution failed");
                let _ = FreeLibrary(h_ntdll);
                return;
            }

            let mut start = 0i64;
            let mut end = 0i64;

            let _ = QueryPerformanceCounter(&mut start);
            Sleep(1);
            let _ = QueryPerformanceCounter(&mut end);

            let elapsed = (end - start) as f64 / freq as f64 * 1000.0;
            total_elapsed += elapsed;
        }

        let avg_elapsed = total_elapsed / iterations as f64;
        let mut cur_res: u32 = 0;
        let mut min_res: u32 = 0;
        let mut max_res: u32 = 0;
        nt_query_timer_resolution(&mut min_res, &mut max_res, &mut cur_res);

        println!(
            "Average over {} iterations: {:.3}ms (Resolution: {:.3}ms, Min: {:.3}ms, Max: {:.3}ms)",
            iterations,
            avg_elapsed,
            cur_res as f64 / 10000.0,
            min_res as f64 / 10000.0,
            max_res as f64 / 10000.0
        );

        let _ = FreeLibrary(h_ntdll);
    }
}

fn main() {
    let _cleanup = CleanupHandler;

    let _mutex_handle = match create_app_mutex() {
        Some(handle) => handle,
        None => {
            println!("Another instance of this application is already running.");
            println!("Please close the other instance first.");
            println!("Press Enter to exit...");
            let mut _input = String::new();
            let _ = io::stdin().read_line(&mut _input);
            return;
        }
    };

    if !is_running_as_admin() {
        println!("This application requires administrator privileges.");
        println!("Please restart the program as administrator.");
        println!("Press Enter to exit...");
        let mut _input = String::new();
        let _ = io::stdin().read_line(&mut _input);
        return;
    }

    loop {
        clear_console();
        println!("1. Set to 1ms (if supported)");
        println!("2. Measure");
        println!("3. Close");
        println!("4. Reset to default (~15.6ms)");
        print!("Select an option (1-4): ");
        let _ = stdout().flush();

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .expect("Failed to read input");

        match input.trim() {
            "1" => {
                set_custom();
                println!("Press Enter to continue...");
                let mut _input = String::new();
                let _ = io::stdin().read_line(&mut _input);
            }
            "2" => {
                measure(100);
                println!("Press Enter to continue...");
                let mut _input = String::new();
                let _ = io::stdin().read_line(&mut _input);
            }
            "3" => {
                println!("Closing application...");
                break;
            }
            "4" => {
                reset_to_default();
                println!("Press Enter to continue...");
                let mut _input = String::new();
                let _ = io::stdin().read_line(&mut _input);
            }
            _ => {
                println!("Invalid option! Please select 1, 2, 3, or 4.");
                println!("Press Enter to continue...");
                let mut _input = String::new();
                let _ = io::stdin().read_line(&mut _input);
            }
        }
    }
}