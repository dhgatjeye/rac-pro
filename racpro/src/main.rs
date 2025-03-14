use std::io::{self, stdout, Write};
use winapi::um::timeapi::{timeBeginPeriod, timeEndPeriod, timeGetDevCaps};
use winapi::um::mmsystem::TIMECAPS;
use winapi::um::profileapi::{QueryPerformanceCounter, QueryPerformanceFrequency};
use winapi::shared::ntdef::{LARGE_INTEGER, NTSTATUS};
use std::mem::size_of;
use winapi::um::securitybaseapi::GetTokenInformation;
use winapi::um::winnt::{TokenElevation, TOKEN_ELEVATION, TOKEN_QUERY};
use winapi::shared::minwindef::DWORD;
use winapi::um::processthreadsapi::{GetCurrentProcess, OpenProcessToken};
use winapi::shared::ntdef::HANDLE;
use winapi::shared::minwindef::ULONG;
use std::ptr::null_mut;
use winapi::um::synchapi::{CreateMutexW, Sleep};
use winapi::um::errhandlingapi::GetLastError;
use winapi::shared::winerror::ERROR_ALREADY_EXISTS;
use crossterm::{terminal::{ClearType, Clear}, QueueableCommand, cursor::{MoveTo, Hide}};

type NtQueryTimerResolution = unsafe extern "system" fn(
    MinimumResolution: *mut ULONG,
    MaximumResolution: *mut ULONG,
    CurrentResolution: *mut ULONG,
) -> NTSTATUS;

struct CleanupHandler;
struct MutexHandle(HANDLE);

impl Drop for CleanupHandler {
    fn drop(&mut self) {}
}

impl Drop for MutexHandle {
    fn drop(&mut self) {
        unsafe {
            winapi::um::handleapi::CloseHandle(self.0);
        }
    }
}

pub fn clear_console() {
    let mut out = stdout();
    out.queue(Hide).unwrap();
    out.queue(Clear(ClearType::All)).unwrap();
    out.queue(MoveTo(0, 0)).unwrap();
    out.flush().unwrap();
}

fn is_running_as_admin() -> bool {
    unsafe {
        let mut token_handle: HANDLE = std::mem::zeroed();
        if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token_handle) == 0 {
            return false;
        }

        let mut elevation: TOKEN_ELEVATION = std::mem::zeroed();
        let mut size: DWORD = size_of::<TOKEN_ELEVATION>() as DWORD;
        let result = GetTokenInformation(
            token_handle,
            TokenElevation,
            &mut elevation as *mut _ as *mut _,
            size,
            &mut size,
        );

        if result != 0 {
            elevation.TokenIsElevated != 0
        } else {
            false
        }
    }
}

fn create_app_mutex() -> Option<MutexHandle> {
    let mutex_name = "Global\\InputLagApplicationMutex";
    let wide_name: Vec<u16> = mutex_name.encode_utf16().chain(std::iter::once(0)).collect();

    unsafe {
        let handle = CreateMutexW(null_mut(), 0, wide_name.as_ptr());
        let last_error = GetLastError();
        if last_error == ERROR_ALREADY_EXISTS {
            None
        } else {
            Some(MutexHandle(handle))
        }
    }
}

fn reset_to_default() -> bool {
    unsafe {
        timeEndPeriod(1);
        let default_period = 16;
        if timeBeginPeriod(default_period) == 0 {
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
        let mut caps: TIMECAPS = std::mem::zeroed();
        if timeGetDevCaps(&mut caps, size_of::<TIMECAPS>() as u32) == 0 {
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
        let lib_name: Vec<u16> = "NtDll.dll".encode_utf16().chain(std::iter::once(0)).collect();
        let h_ntdll = winapi::um::libloaderapi::LoadLibraryW(lib_name.as_ptr());
        if h_ntdll.is_null() {
            println!("LoadLibrary failed");
            return;
        }

        let lib = libloading::Library::new("NtDll.dll").unwrap();
        let nt_query_timer_resolution = match lib.get::<NtQueryTimerResolution>(b"NtQueryTimerResolution\0") {
            Ok(func) => Some(*func),
            Err(_) => {
                println!("Failed to load NtQueryTimerResolution");
                None
            }
        };

        if let Some(func) = nt_query_timer_resolution {
            let mut freq: LARGE_INTEGER = std::mem::zeroed();
            QueryPerformanceFrequency(&mut freq);

            let mut total_elapsed = 0.0;

            for _ in 0..iterations {
                let mut min_res: ULONG = 0;
                let mut max_res: ULONG = 0;
                let mut cur_res: ULONG = 0;

                if func(&mut min_res, &mut max_res, &mut cur_res) != 0 {
                    println!("NtQueryTimerResolution failed");
                    winapi::um::libloaderapi::FreeLibrary(h_ntdll);
                    return;
                }

                let mut start: LARGE_INTEGER = std::mem::zeroed();
                let mut end: LARGE_INTEGER = std::mem::zeroed();

                QueryPerformanceCounter(&mut start);
                Sleep(1);
                QueryPerformanceCounter(&mut end);

                let elapsed = (*end.QuadPart() - *start.QuadPart()) as f64 / *freq.QuadPart() as f64 * 1000.0;
                total_elapsed += elapsed;
            }

            let avg_elapsed = total_elapsed / iterations as f64;
            let mut cur_res: ULONG = 0;
            let mut min_res: ULONG = 0;
            let mut max_res: ULONG = 0;
            func(&mut min_res, &mut max_res, &mut cur_res);

            println!(
                "Average over {} iterations: {:.3}ms (Resolution: {:.3}ms, Min: {:.3}ms, Max: {:.3}ms)",
                iterations,
                avg_elapsed,
                cur_res as f64 / 10000.0,
                min_res as f64 / 10000.0,
                max_res as f64 / 10000.0
            );
        }

        winapi::um::libloaderapi::FreeLibrary(h_ntdll);
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
            io::stdin().read_line(&mut _input).unwrap();
            return;
        }
    };

    if !is_running_as_admin() {
        println!("This application requires administrator privileges.");
        println!("Please restart the program as administrator.");
        println!("Press Enter to exit...");
        let mut _input = String::new();
        io::stdin().read_line(&mut _input).unwrap();
        return;
    }

    loop {
        clear_console();
        println!("1. Set to 1ms (if supported)");
        println!("2. Measure");
        println!("3. Close");
        println!("4. Reset to default (~15.6ms)");
        print!("Select an option (1-4): ");
        stdout().flush().unwrap();

        let mut input = String::new();
        io::stdin()
            .read_line(&mut input)
            .expect("Failed to read input");

        match input.trim() {
            "1" => {
                set_custom();
                println!("Press Enter to continue...");
                let mut _input = String::new();
                io::stdin().read_line(&mut _input).unwrap();
            }
            "2" => {
                measure(100);
                println!("Press Enter to continue...");
                let mut _input = String::new();
                io::stdin().read_line(&mut _input).unwrap();
            }
            "3" => {
                println!("Closing application...");
                break;
            }
            "4" => {
                reset_to_default();
                println!("Press Enter to continue...");
                let mut _input = String::new();
                io::stdin().read_line(&mut _input).unwrap();
            }
            _ => {
                println!("Invalid option! Please select 1, 2, 3, or 4.");
                println!("Press Enter to continue...");
                let mut _input = String::new();
                io::stdin().read_line(&mut _input).unwrap();
            }
        }
    }
}