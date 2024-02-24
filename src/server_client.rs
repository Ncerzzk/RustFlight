use std::{path::Path, io::{self, Read, BufReader, BufRead, Write}, os::{unix::net::{UnixListener, UnixStream}, fd::AsRawFd}, ffi::CStr, sync::LazyLock, mem::MaybeUninit, collections::HashMap};
use polling::{Poller, Event, Events};

use crate::{module::Module, pthread_scheduler::SchedulePthread};

#[repr(C)]
struct ThreadSpecificData  {
    stream:*mut UnixStream
}

static PTHREAD_KEY:LazyLock<u32>= LazyLock::new(||{
    let mut key:MaybeUninit<u32> = MaybeUninit::zeroed();
    unsafe{
        libc::pthread_key_create(key.as_mut_ptr(), Some(drop_specifidata));
        key.assume_init()
    }
});

fn set_thread_specifidata(data:ThreadSpecificData){
    let data = Box::new(data);
    let data = Box::leak(data);
    unsafe{
        libc::pthread_setspecific(*PTHREAD_KEY, &*data as *const ThreadSpecificData as *const libc::c_void );
    } 
}

unsafe extern "C" fn drop_specifidata(ptr:*mut libc::c_void){
    unsafe{
        drop(Box::from_raw(ptr as *mut ThreadSpecificData));
    };
}


pub fn server_init<P: AsRef<Path>>(socket_path:P)->Result<(), std::io::Error>{
    let _ = std::fs::remove_file(socket_path.as_ref());

    let listener = UnixListener::bind(socket_path.as_ref()).unwrap();
    listener.set_nonblocking(true).unwrap();

    let poller = Poller::new().unwrap();
    let mut fd_thread_map:HashMap<usize,libc::c_ulong> = HashMap::new();
    loop{
        if let Ok((mut client,_)) = listener.accept(){
            let mut buffer = [0; 100];
            client.read(&mut buffer).unwrap();
    
            let cmd_raw= CStr::from_bytes_until_nul(&buffer).unwrap().to_str().unwrap().to_string();
            if cmd_raw.contains("shutdown"){
                break;
            }
    
            let mut client_cp = client.try_clone().unwrap();
            let x= SchedulePthread::new_simple(Box::new(move ||{
                let cmd_with_args:Vec<_> = cmd_raw.split_whitespace().collect();
                assert!(cmd_with_args.len()>=1);
    
                let data =ThreadSpecificData{
                    stream: &mut client_cp as *mut UnixStream,
                };
                set_thread_specifidata(data);
                Module::get_module(cmd_with_args[0]).execute((cmd_with_args.len()) as u32, cmd_with_args.as_ptr());
                _ = client_cp.shutdown(std::net::Shutdown::Both);
            }));
            unsafe { poller.add(&client, Event::none(client.as_raw_fd() as usize).with_interrupt()).unwrap() };
            fd_thread_map.insert(client.as_raw_fd() as usize, x.thread_id);
        }

        let mut events = Events::new();
        let _ = poller.wait(&mut events, Some(std::time::Duration::from_secs(1)));

        for ev in events.iter(){
            let thread = fd_thread_map.remove(&ev.key).unwrap();
            unsafe{
                libc::pthread_cancel(thread); // some memory may leak
            }
        }


    }
        
    Ok(())
}


pub struct Client{
    stream:UnixStream 
}

impl Client{
    pub fn new<P: AsRef<Path>>(socket_path:P)->Result<Client,io::Error>{
        let stream = UnixStream::connect(socket_path.as_ref())?;
        let client = Client{
            stream,
        };
        Ok(client)
    }

    pub fn block_read(&mut self){
        let mut bufreader = BufReader::new(self.stream.try_clone().unwrap());
        let mut str_out:String = String::new();
        while let Ok(n) = bufreader.read_line(&mut str_out){
            if n == 0{break;}
            print!("{}",str_out);
            str_out.clear();
        }
    }

    pub fn send_str(&mut self,data:&str){
        self.stream.write_all(data.as_bytes()).unwrap();
        self.stream.flush().unwrap();
    }
}

#[macro_export]
macro_rules! thread_logln {
    ($($arg:tt)*) => {
        write!(rpos::server_client::get_output(),"{}\n", format!($($arg)*)).unwrap()
    }
}

#[macro_export]
macro_rules! thread_log {
    ($($arg:tt)*) => {
        write!(rpos::server_client::get_output(),"{}", format!($($arg)*)).unwrap()
    }
}

pub fn get_output()->Box<dyn Write>{
    let thread_data = unsafe { libc::pthread_getspecific(*PTHREAD_KEY) };
    if thread_data == std::ptr::null_mut(){
        Box::new(std::io::stdout()) as Box<dyn Write>
    }else{
        let stream:&ThreadSpecificData = unsafe{&mut *(thread_data as *mut ThreadSpecificData)};
        unsafe{
            Box::new((*stream.stream).try_clone().unwrap()) as Box<dyn Write>
        }
    }
}