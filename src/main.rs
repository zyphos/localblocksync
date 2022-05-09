/*
 This program is free software: you can redistribute it and/or modify it under
 the terms of the GNU General Public License as published by the Free Software
 Foundation, either version 3 of the License, or (at your option) any later
 version.

 This program is distributed in the hope that it will be useful,
 but WITHOUT ANY WARRANTY; without even the implied warranty of
 MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.
 See the GNU General Public License for more details.

 You should have received a copy of the GNU General Public License along with
 this program. If not, see <https://www.gnu.org/licenses/>.
*/
use std::sync::{Arc, Mutex};
use std::{fs,thread,process};
use std::path::Path;
use nix::ioctl_read;
use block_utils;
use std::os::unix::io::AsRawFd;
use std::io::{prelude::*, stdout};
use std::fs::{File, OpenOptions};
use std::os::unix::fs::FileExt;
use std::time::Instant;
use nix::libc::ftruncate64;
use clap::Parser;

/// Sync file and block device that write only difference
#[derive(Parser, Debug)]
#[clap(author, version, about, long_about = None)]
struct Args {
    /// Use 2 threads to read source and destination at the same time. Do not use if they are on the same physical disk.
    #[clap(short, long)]
    thread: bool,

    /// Read buffer size in MB, default 100MB (Need 2x this in RAM)
    #[clap(short, long, default_value_t = 100)]
    buffer: usize,

    /// Read buffer size in KB, default 1MB (Need 1x this in RAM)
    #[clap(short, long, default_value_t = 1024)]
    chunck: usize,

    /// Quiet mode, do not print interactive user detail like: show progress, Threaded mode active, Buffer Size, Block size, filesize
    #[clap(short, long)]
    quiet: bool,

    /// Path of data source, a file or a block device
    src_path: String,

    /// Path of data destination, a file or a block device
    dst_path: String,
}

// Generate ioctl function
const BLKGETSIZE64_CODE: u8 = 0x12; // Defined in linux/fs.h
const BLKGETSIZE64_SEQ: u8 = 114;
ioctl_read!(ioctl_blkgetsize64, BLKGETSIZE64_CODE, BLKGETSIZE64_SEQ, u64); // Define function ioctl_blkgetsize64

fn main(){
    let arg = Args::parse();
    let src_path = Path::new(&arg.src_path);
    let dst_path = Path::new(&arg.dst_path);
    copy(src_path, dst_path, arg.thread, arg.buffer, arg.chunck, arg.quiet);
}

/// Determine block device size
fn get_device_size(path: &str) -> u64 {
   let file = OpenOptions::new()
             .read(true)
             .open(path).unwrap();

   let fd = file.as_raw_fd();

   let mut cap = 0u64;
   let cap_ptr = &mut cap as *mut u64;

   unsafe {
      ioctl_blkgetsize64(fd, cap_ptr).unwrap();
   }
   return cap;
}

fn is_block_device(path: &std::path::Path) -> bool {
    let path_abs = fs::canonicalize(path).unwrap();

    return match block_utils::is_block_device(path_abs){
        Ok(is_block_device) => is_block_device,
        Err(_e) => false,
    };
}

fn filesize(path: &std::path::Path) -> Result<u64, std::io::Error> {
    match fs::canonicalize(path){
        Ok(path_abs) => {
            if is_block_device(&path_abs) {
                //println!("{:?} is a block device", path_abs);
                return Ok(get_device_size(path_abs.to_str().unwrap()));
            }
            match path_abs.metadata(){
                Ok(m) => Ok(m.len()),
                Err(_e) => Ok(0)
            }
        },
        Err(_e) => Ok(0)
    }
}

fn display_progress(file_cursor_pos: f64, src_size: f64, start_time: Instant){
    let mut stdout = stdout();
    let progress = file_cursor_pos / src_size;
    let progress_round = (progress*10.).ceil();
    let progress_txt = format!("{:#<1$}","", progress_round as usize);
    let progress_pc = (progress * 100.).ceil();
    let speed_mb = file_cursor_pos / start_time.elapsed().as_secs() as f64 / 1024. / 1024.;
    let remaining_time = start_time.elapsed().as_secs() as f64 * (src_size - file_cursor_pos) / file_cursor_pos;
    print!("\r[{:-<10}] {}% - {:.3} MB/s - Remaining {:.0}s          ", progress_txt, progress_pc, speed_mb, remaining_time);
    stdout.flush().unwrap();
}

fn copy(src_path: &Path, dst_path: &Path, threaded: bool, buffer_size: usize, chunck_size: usize, quiet: bool){
    println!("Synching {:?} to {:?}", src_path, dst_path);
    let src_size = filesize(src_path).unwrap();
    let dst_size = filesize(dst_path).unwrap();
    if !quiet{
        println!("Sizes:");
        println!("{}: {} [{:.1} MB]", src_path.to_str().unwrap(), src_size, src_size as f64 / 1024. / 1024.);
        println!("{}: {} [{:.1} MB]", dst_path.to_str().unwrap(), dst_size, dst_size as f64 / 1024. / 1024.);
    }

    if src_size == 0{
        println!("Source file is empty ! Nothing to do !");
        return;
    }

    let mut src_file = match File::open(src_path){
        Ok(src_file) => src_file,
        Err(_err) => {
            println!("Failed to open {}.", src_path.to_str().unwrap());
            process::exit(1);
        }
    };
    let mut dst_file = match OpenOptions::new().create(true).read(true).write(true).open(dst_path){
        Ok(dst_file) => dst_file,
        Err(_err) => {
            println!("Failed to open {} in write mode.", dst_path.to_str().unwrap());
            process::exit(1);
        }
    };

    if dst_size != src_size && !is_block_device(dst_path){
        println!("Truncate {:?} from {} to {} bytes", dst_path, dst_size, src_size);
        unsafe{
            ftruncate64(dst_file.as_raw_fd(), src_size as i64);
        }
    } else if is_block_device(dst_path) && dst_size < src_size{
        println!("Destination is a block device and is too small.");
        return;
    }

    let buffer_size: usize = 1024*1024*buffer_size;
    let block_size: usize = 1024*chunck_size; // Window for writing
    if !quiet{
        println!("Buffer size: 2x {} [{:.1} MB]", buffer_size, buffer_size as f64 / 1024. / 1024.);
        println!("Block size (chunk): 2x {} [{:.1} MB]", block_size, block_size as f64 / 1024. / 1024.);
    }

    let mut buffer_src = vec![0u8; buffer_size];
    let mut buffer_dst = vec![0u8; buffer_size];
    let mut fp: usize = 0;
    let mut bytes_written: usize = 0;
    let mut time2display = Instant::now();
    let start_time = Instant::now();

    if threaded{
        if !quiet {
            println!("Threaded - Reading source and destination at the same time.");
        }
        let src_file = Arc::new(Mutex::new(src_file));
        let buffer_src = Arc::new(Mutex::new(buffer_src));
        let src_len = Arc::new(Mutex::new(0));

        loop{
            let src_file = Arc::clone(&src_file);
            let buffer_src1 = Arc::clone(&buffer_src);
            let buffer_src2 = Arc::clone(&buffer_src);
            let src_len1 = Arc::clone(&src_len);
            let src_len2 = Arc::clone(&src_len);
            let thandle = thread::spawn(move || {
                let mut src_len = src_len1.lock().unwrap();
                let mut src_file = src_file.lock().unwrap();
                let mut buffer_src = buffer_src1.lock().unwrap();
                *src_len = (*src_file).read(&mut *buffer_src).unwrap();
            });

            let dst_len = dst_file.read(&mut buffer_dst).unwrap();
            // Wait thread to finish
            thandle.join().unwrap();

            let src_len = src_len2.lock().unwrap();
            let buffer_src = buffer_src2.lock().unwrap();
            
            if *src_len == 0 || dst_len == 0{
                break;
            }
            if *src_len != dst_len{
                println!("Read len are not equal !");
                break;
            }
            if *buffer_src != buffer_dst{
                let mut block_start_pos = 0;
                let mut block_pos = 0;
                let mut current_block_differ = false;
                loop{
                    let mut block_size = block_size;
                    if block_size + block_pos > *src_len{
                        block_size = *src_len - block_pos;
                        if block_size <= 0{
                            if current_block_differ{
                                dst_file.write_at(&buffer_src[block_start_pos .. block_pos], fp as u64 + block_start_pos as u64).unwrap();
                                bytes_written += block_pos - block_start_pos;
                            }
                            break;
                        }
                    }
                    let next_block_pos = block_pos + block_size;
                    if buffer_src[block_pos .. next_block_pos] != buffer_dst[block_pos .. next_block_pos]{
                        if !current_block_differ{
                            block_start_pos = block_pos;
                            current_block_differ = true;
                        }
                    }else{
                        if current_block_differ{
                            dst_file.write_at(&buffer_src[block_start_pos .. block_pos], fp as u64 + block_start_pos as u64).unwrap();
                            bytes_written += block_pos - block_start_pos;
                            current_block_differ = false;
                        }
                    }
                    block_pos = next_block_pos;
                }
            }
            fp += *src_len;
            if !quiet && time2display.elapsed().as_secs() > 2{
                display_progress(fp as f64, src_size as f64, start_time);
                time2display = Instant::now();
            }
        }
    }else{
        loop{
            let src_len = src_file.read(&mut buffer_src).unwrap();
            let dst_len = dst_file.read(&mut buffer_dst).unwrap();
            if src_len == 0 || dst_len == 0{
                break;
            }
            if src_len != dst_len{
                println!("Read len are not equal !");
                break;
            }
            if buffer_src != buffer_dst{
                dst_file.write_at(&buffer_src[0 .. src_len], fp as u64).unwrap();
                bytes_written += src_len;
            }
            fp += src_len;
            if !quiet && time2display.elapsed().as_secs() > 2{
                    display_progress(fp as f64, src_size as f64, start_time);
                time2display = Instant::now();
            }
        }
    }
    if !quiet{
        println!(""); // To skip line after display_progress
    }
    println!("Elapsed time: {:.2}s", start_time.elapsed().as_secs());
    println!("Total bytes written: {} [{:.1} MB]", bytes_written, bytes_written as f64 / 1024. / 1024.);
}
