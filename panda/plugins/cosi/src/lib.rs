use std::mem::size_of;
use std::sync::atomic::{AtomicBool, Ordering};

use downloader::get_symtab_name;
use panda::mem::read_guest_type;

use panda::prelude::*;

use once_cell::sync::{Lazy, OnceCell};
use volatility_profile::VolatilityJson;

//use panda::plugins::cosi::osi_static;

use panda::plugins::syscalls2::Syscalls2Callbacks;

static SYMBOL_TABLE: OnceCell<VolatilityJson> = OnceCell::new();

mod downloader;
/// Interface for other plugins to interact with
mod ffi;
mod kaslr;
mod process;
mod structs;

use kaslr::kaslr_offset;

use std::path::{Path, PathBuf};

use crate::downloader::download_symbol_table;
use crate::structs::CosiProc;

#[derive(PandaArgs)]
#[name = "cosi"]
struct Args {
    #[arg(about = "Path to a volatility 3 symbol table to use (.xz compressed json)")]
    profile: String,
}

static ARGS: Lazy<Args> = Lazy::new(Args::from_panda_args);

#[allow(deprecated)]
fn symbol_table() -> &'static VolatilityJson {
    SYMBOL_TABLE.get_or_init(|| {
        let name = get_symtab_name();

        let path = if ARGS.profile.is_empty() || !Path::new(&ARGS.profile).exists() {
            let filename = if ARGS.profile.is_empty() {
                &name
            } else {
                &ARGS.profile
            };

            let home = std::env::home_dir().unwrap();
            // This part is hacky and bad, but PathBuf::push() was choking on something
            // (probably the many '.'s in the symbol table name), whereas this seems to work
            // so it's like this until I'm back from break :)
            let path_name = home.to_str().unwrap().to_owned() + "/.panda/" + filename + ".json.xz";
            let path = std::path::PathBuf::from(path_name);

            if !path.exists() {
                if !path.parent().map(Path::exists).unwrap_or(true) {
                    std::fs::create_dir_all(path.parent().unwrap())
                        .expect("Failed to create panda directory");
                }

                println!("Given symbol table not found, attempting to download...");
                match download_symbol_table(
                    path.to_str().unwrap(),
                    path.file_name().as_ref().unwrap().to_str().unwrap(),
                ) {
                    true => println!("Downloaded!"),
                    false => {
                        println!("Download failed, exiting");
                        std::process::exit(1)
                    }
                };
            }

            path
        } else {
            PathBuf::from(&ARGS.profile)
        };

        VolatilityJson::from_compressed_file(&path)
    })
}

static READY_FOR_KASLR_SEARCH: AtomicBool = AtomicBool::new(false);

#[panda::init]
fn init(_: &mut PluginHandle) -> bool {
    // Ensure symbol table is initialized
    let _ = symbol_table();

    let first_syscall = panda::PppCallback::new();

    first_syscall.on_all_sys_enter(move |_, _, _| {
        READY_FOR_KASLR_SEARCH.store(true, Ordering::SeqCst);

        first_syscall.disable();
    });

    true
}

#[panda::uninit]
fn exit(_: &mut PluginHandle) {
    println!("Exiting");
}

fn current_cpu_offset(cpu: &mut CPUState) -> target_ulong {
    let symbol_table = symbol_table();

    let cpu_offset = match symbol_table.symbol_from_name("__per_cpu_offset") {
        Some(symbol) => symbol.address as target_ptr_t,
        None => return 0,
    };

    let kaslr_offset = kaslr_offset(cpu);
    let cpu_num = cpu.cpu_index as target_ptr_t;
    let offset_in_array = size_of::<target_ulong>() as target_ptr_t * cpu_num;

    let cpu_offset_ptr = kaslr_offset + cpu_offset + offset_in_array;
    let cpu_offset: target_ulong = read_guest_type(cpu, cpu_offset_ptr).unwrap();

    cpu_offset
}

/*
osi_static! {
    #[per_cpu]
    #[symbol = "current_task"]
    static CURRENT_TASK: TaskStruct;
} */

// Currently walks the process list based on task_struct->tasks,
// but some systems might instead have task_struct->next_task field
// Possibly we don't need to do all the work of locating the
/// `get_process_list` returns a `Vec` of `CosiProc`s representing all processes currently running on the system, starting from `init`
fn get_process_list(cpu: &mut CPUState) -> Option<Vec<CosiProc>> {
    let mut ret = Vec::<CosiProc>::new();
    let mut ts_current = match CosiProc::get_init_cosiproc(cpu) {
        Some(res) => res,
        None => match CosiProc::get_current_cosiproc(cpu) {
            Some(res) => {
                let tmp = CosiProc::new(cpu, res.taskd)?;
                tmp.get_next_process(cpu)?
            }
            None => return None,
        },
    };
    let first_addr = ts_current.addr;
    loop {
        ret.push(ts_current.clone());
        ts_current = match ts_current.get_next_process(cpu) {
            Some(next) => next,
            None => break,
        };
        if ts_current.addr == 0 || ts_current.addr == first_addr {
            break;
        }
    }
    Some(ret)
}

/// `get_process_children` returns a `Vec` of `CosiProcs` representing all the children of the process represented by a given `CosiProc`
fn get_process_children(cpu: &mut CPUState, proc: &CosiProc) -> Option<Vec<CosiProc>> {
    let mut ret = Vec::<CosiProc>::new();
    let mut ts_current = proc.get_next_child(cpu)?;
    let first_addr = ts_current.addr;
    println!("First addr: {first_addr:x} | proc_addr: {:x}", proc.addr);
    loop {
        ret.push(ts_current.clone());
        ts_current = match ts_current.get_next_sibling(cpu) {
            Some(next) => next,
            None => {
                println!("Goofed it");
                break;
            }
        };
        if ts_current.addr == 0 || ts_current.addr == first_addr {
            break;
        }
    }
    Some(ret)
}
//#[allow(dead_code)]
//fn print_current_cosiproc_info(cpu: &mut CPUState) -> bool {
//    match CosiProc::get_current_cosiproc(cpu) {
//        Some(res) => {
//            if res.asid != 0 {
//                println!("asid: {:x}", res.asid);
//            } else {
//                println!("asid: Err");
//            }
//            println!("start_time: {:x}", res.task.start_time);
//            println!("name: {}", res.name);
//            println!("pid, {:x}", res.task.pid);
//            println!("ppid, {:x}", res.ppid);
//            println!("taskd, {:x}", res.taskd);
//        }
//        None => println!("Could not read current proc"),
//    };
//    true
//}
//
//#[allow(dead_code)]
//fn print_current_cosithread_info(cpu: &mut CPUState) -> bool {
//    match CosiThread::get_current_cosithread(cpu) {
//        Some(res) => {
//            println!("tid: {:x}", res.tid);
//            println!("pid: {:x}", res.pid);
//        }
//        None => println!("Could not read current proc"),
//    };
//    true
//}
//
//#[allow(dead_code)]
//fn print_current_cosifile_info(cpu: &mut CPUState) -> bool {
//    match CosiFiles::get_current_files(cpu) {
//        Some(res) => {
//            if let Some(fd1) = res.file_from_fd(1) {
//                println!("fd 1 name: {}", fd1.name);
//            }
//
//            for i in res.files {
//                println!("file name: {} | fd: {}", i.name, i.fd);
//            }
//        }
//        None => println!("Could not read files from current proc"),
//    }
//    true
//}
//
//#[allow(dead_code)]
//fn print_current_cosimappings_info(cpu: &mut CPUState) -> bool {
//    match CosiProc::get_current_cosiproc(cpu) {
//        Some(res) => match res.get_mappings(cpu) {
//            Some(mapping) => {
//                for mdl in mapping.modules.iter() {
//                    println!(
//                        "modd: {:x} | base: {:x} | size: {:x} | file: {} | name: {}",
//                        mdl.modd, mdl.base, mdl.size, mdl.file, mdl.name
//                    )
//                }
//            }
//            None => println!("Could not read memory mapping"),
//        },
//        None => println!("Could not read current process"),
//    }
//    true
//}
//
//#[allow(dead_code)]
//fn print_process_list(cpu: &mut CPUState) -> bool {
//    match get_process_list(cpu) {
//        Some(res) => {
//            for i in res.iter() {
//                println!("name: {} | pid: {}", i.name, i.task.pid);
//            }
//        }
//        None => println!("No process list found"),
//    };
//
//    true
//}
//
//fn print_children(cpu: &mut CPUState) -> bool {
//    match CosiProc::get_current_cosiproc(cpu) {
//        Some(proc) => {
//            println!(
//                "[current] name: {} | pid: {} | ppid: {} | addr: {:x}",
//                proc.name, proc.task.pid, proc.ppid, proc.addr
//            );
//            match get_process_children(cpu, &proc) {
//                Some(children) => {
//                    for c in children.iter() {
//                        println!(
//                            "\t [child] name: {} | pid: {} | ppid: {}",
//                            c.name, c.task.pid, c.ppid
//                        );
//                    }
//                    std::process::exit(0);
//                }
//                None => println!("No Children (2003)"),
//            }
//        }
//        None => println!("Could not get current process"),
//    };
//    true
//}
//static mut download: bool = false;
//
//#[panda::asid_changed]
//fn asid_changed(cpu: &mut CPUState, _old_asid: target_ulong, _new_asid: target_ulong) -> bool {
//    //println!("\n\nOSI2 INFO START");
//
//    // Manually testing the downloader functionality
//    if unsafe { download } {
//        println!("Downloading...");
//        match download_symbol_table("look_at_me", "ubuntu:3.4.0-4-goldfish:32") {
//            true => println!("Downloaded!"),
//            false => {
//                println!("Download failed, exiting");
//                std::process::exit(1)
//            }
//        }
//        unsafe {
//            download = false;
//        }
//    }
//
//    //print_current_cosiproc_info(cpu);
//    //print_current_cosithread_info(cpu);
//    //print_current_cosifile_info(cpu);
//    //print_current_cosimappings_info(cpu);
//    //print_process_list(cpu);
//    //print_children(cpu);
//
//    //println!("OSI2 INFO END\n\n");
//
//    true
//}
