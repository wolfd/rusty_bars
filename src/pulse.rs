#![allow(unstable)]
#![allow(dead_code)]


extern crate libc;
use libc::funcs::c95::string::strlen;
use self::libc::{c_int, c_char, c_void, size_t};
use std::ffi::CString;
pub use pulse_types::*;
use std::ptr;
use std::mem;
use std::fmt;
use std::slice;
use std::sync::{Arc, Mutex};
use std::io::{Reader, Writer, IoResult, IoError, IoErrorKind};


// Types for callback closures
type StateCallback = Fn(Context, pa_context_state) + Send;
type ServerInfoCallback = Fn(Context, &pa_server_info) + Send;
type SinkInfoCallback = Fn(Context, Option<&pa_sink_info>) + Send;

// Boxed types for callback closures
type BoxedStateCallback = Box<StateCallback>;
type BoxedServerInfoCallback = Box<ServerInfoCallback>;
type BoxedSinkInfoCallback = Box<SinkInfoCallback>;


/// Coverts a pulse error code to a String
fn pa_err_to_string(err: c_int) -> Result<(), String> {
    if err == 0 {
        Ok(())
    } else {
        unsafe {
            let err_msg_ptr: *const c_char = ext::pa_strerror(err);
            let size = strlen(err_msg_ptr) as usize;
            let slice: Vec<u8> = Vec::from_raw_buf((err_msg_ptr as *const u8), size);
            Err(String::from_utf8(slice).unwrap())
        }
    }
}


/// A safe wrapper for pa_context_connect
pub fn pa_context_connect(context: *mut opaque::pa_context, server_name: Option<&str>,
    flags: enums::pa_context_flags, spawn_api: Option<*const opaque::pa_spawn_api>) {

    assert!(!context.is_null());

    let server: *const c_char = match server_name {
        None => ptr::null(),
        Some(name) => CString::from_slice(name.as_bytes()).as_ptr()
    };

    let spawn_api_ptr: *const opaque::pa_spawn_api = match spawn_api {
        Some(api_ptr) => {
            assert!(!api_ptr.is_null());
            api_ptr
        },
        None => ptr::null()
    };

    let res = unsafe { ext::pa_context_connect(context, server, flags, spawn_api_ptr) };
    assert!(res == 0);
}



/// A struct which wraps the PulseAudio async main loop.
pub struct PulseAudioMainloop {
    internal: *mut pa_mainloop
}


impl PulseAudioMainloop {
    /// Create a new mainloop.
    pub fn new() -> PulseAudioMainloop {
        PulseAudioMainloop{
            internal: pa_mainloop_new()
        }
    }

    /// Helper method to get the raw mainloop api
    fn get_raw_mainloop_api(&self) -> *mut pa_mainloop_api {
        pa_mainloop_get_api(self.internal)
    }

    //// Creates a new context with this mainloop
    pub fn create_context(&self, client_name: &str) -> Context {
        Context::new(self, client_name)
    }

    /// Run the mainloop.
    pub fn run(&self) {
        let mut mainloop_res: c_int = 0;
        pa_mainloop_run(self.internal, &mut mainloop_res);
        // TODO: error handling
    }
}


unsafe impl Send for ContextInternal {}



#[derive(Clone)]
pub struct Context {
    internal: Arc<Mutex<ContextInternal>>
}


impl Context {
    /// Get a new PulseAudio context. It's probably easier to get this via the
    /// mainloop.
    pub fn new(mainloop: &PulseAudioMainloop, client_name: &str) -> Context{
        let context = Context {
            internal: Arc::new(Mutex::new(ContextInternal::new(mainloop, client_name))),
        };
        {
            let internal_guard = context.internal.lock();
            let mut internal = internal_guard.unwrap();
            internal.external = Some(context.clone());
        }
        context
    }

    /// Get a mutable raw pointer to this object
    fn as_mut_ptr(&mut self) -> *mut Context {
        self
    }

    /// Get a mutable void pointer to this object
    fn as_void_ptr(&mut self) -> *mut c_void {
        self.as_mut_ptr() as *mut c_void
    }

    /// Set the callback for server state. This callback gets called many times.
    /// Do not start sending commands until this returns pa_context_state::READY
    pub fn set_state_callback<C>(&mut self, cb: C) where C: Fn(Context, pa_context_state) + Send {
        let internal_guard = self.internal.lock();
        let mut internal = internal_guard.unwrap();
        internal.state_cb = Some(Box::new(cb) as BoxedStateCallback);
        pa_context_set_state_callback(internal.ptr, _state_callback, internal.as_void_ptr());
    }

    /// Connect to the server
    /// 1. Before calling this, you probably want to run set_state_callback
    /// 2. After setting a state callback, run this method
    /// 3. Directly after running this method, start the mainloop to start
    ///    getting callbacks.
    pub fn connect(&self, server: Option<&str>, flags: pa_context_flags) {
        let internal_guard = self.internal.lock();
        let internal = internal_guard.unwrap();
        pa_context_connect(internal.ptr, server, flags, None);
    }

    /// Gets basic information about the server. See the pa_server_info struct
    /// for more details.
    pub fn get_server_info<C>(&self, cb: C) where C: Fn(Context, &pa_server_info), C: Send {
        let internal_guard = self.internal.lock();
        let mut internal = internal_guard.unwrap();
        internal.server_info_cb = Some(Box::new(cb) as BoxedServerInfoCallback);
        pa_context_get_server_info(internal.ptr, _server_info_callback, internal.as_void_ptr());
    }

    pub fn get_sink_info_by_name<C>(&self, name: &str, cb: C) where C: Fn(Context, Option<&pa_sink_info>), C: Send {
        let internal_guard = self.internal.lock();
        let mut internal = internal_guard.unwrap();
        internal.sink_info_cb = Some(Box::new(cb) as BoxedSinkInfoCallback);
        pa_context_get_sink_info_by_name(internal.ptr, name, _sink_info_callback, internal.as_void_ptr());
    }
}


struct ContextInternal {
    /// A pointer to the pa_context object
    ptr: *mut pa_context,
    /// A pointer to our external API
    external: Option<Context>,
    /// Callback closure for state changes. Called every time the state changes
    state_cb: Option<BoxedStateCallback>,
    /// Callback closure for get_server_info. Called once per execution of
    /// get_server_info.
    server_info_cb: Option<BoxedServerInfoCallback>,
    /// Callback closure for getting sink info.  Called once for for each
    /// element in the list of sinks
    sink_info_cb: Option<BoxedSinkInfoCallback>
}


/// Currently the drop method has nothing to trigger it. Need to figure out a
/// game plan here.
impl Drop for ContextInternal {
    fn drop(&mut self) {
        println!("drop internal");
    }
}


impl ContextInternal {
    /// Never invoke directly. Always go through Context
    fn new(mainloop: &PulseAudioMainloop, client_name: &str) -> ContextInternal {
        ContextInternal{
            ptr: pa_context_new(mainloop.get_raw_mainloop_api(), client_name),
            // TODO: deal with dropping circular reference to external with the Arc
            external: None,
            state_cb: None,
            server_info_cb: None,
            sink_info_cb: None,
        }
    }

    /// Gets a new clone of the external API
    fn get_new_external(&self) -> Context {
        self.external.clone().unwrap()
    }

    /// Get the current context state. This function is synchronous.
    fn get_state(&self) -> pa_context_state {
        pa_context_get_state(self.ptr)
    }

    /// Get a c_void pointer to this object
    fn as_void_ptr(&mut self) -> *mut c_void {
        self.as_mut_ptr() as *mut c_void
    }

    /// Get a mutable raw pointer to this object
    fn as_mut_ptr(&mut self) -> *mut ContextInternal {
        self
    }


    /// Called back for state changes. Wraps the user's closure
    fn state_callback(&self) {
        let state = self.get_state();
        let external = self.external.clone().unwrap();
        match self.state_cb {
            Some(ref cb) => cb(external, state),
            None => println!("warning: no context state callback set")
        }
    }

    /// Called back for get_server_info. Wraps the user's closure
    fn server_info_callback(&self, info: &pa_server_info) {
        let external = self.external.clone().unwrap();
        match self.server_info_cb {
            Some(ref cb) => cb(external, info),
            None => println!("warning: no server info callback is set"),
        }
    }

    /// Called back for the sink_info_list and get_sink_info commands
    fn sink_info_callback(&self, info: Option<&pa_sink_info>) {
        let external = self.external.clone().unwrap();
        match self.sink_info_cb {
            Some(ref cb) => cb(external, info),
            None => println!("warning: no sink info callback is set"),
        }
    }

}

/// Utility to convert C strings to String objects
/// TODO: standard library
pub fn cstr_to_string(c_str: *const c_char) -> String {
    let len: usize = unsafe{ strlen(c_str) } as usize;
    let s = unsafe{ String::from_raw_parts(c_str as *mut u8, len, len) };
    let retval = s.clone();
    unsafe{ mem::forget(s) };
    retval
}

/// State callback for C to call. Takes a ContextInternal and calls its
/// server_info_callback method.
extern fn _state_callback(_: *mut pa_context, context: *mut c_void) {
    let context_internal = unsafe{ &* (context as *mut ContextInternal) };
    context_internal.state_callback();
}

/// Server info callback for C to call. Takes a ContextInternal and calls its
/// server_info_callback method.
extern fn _server_info_callback(_: *mut pa_context, info: *const pa_server_info, context: *mut c_void) {
    let context_internal = unsafe{ &* (context as *mut ContextInternal) };
    context_internal.server_info_callback(unsafe{ &*info });
}


extern fn _sink_info_callback(_: *mut pa_context, info: *const pa_sink_info, eol: c_int, context: *mut c_void) {
    let context_internal = unsafe{ &* (context as *mut ContextInternal) };
    if eol == 1 || info.is_null() {
        context_internal.sink_info_callback(None);
    } else {
        context_internal.sink_info_callback(Some(unsafe{ &*info }));
    }

}


/// A safe interface to pa_context_set_state_callback
pub fn pa_context_set_state_callback(context: *mut opaque::pa_context,
    cb: cb::pa_context_notify_cb_t, userdata: *mut c_void) {
    assert!(!context.is_null());
    unsafe { ext::pa_context_set_state_callback(context, cb, userdata) };
}

/// A rust wrapper around pa_context_disconnect.
/// Immediately/synchronously disconnect from the PulseAudio server.
pub fn pa_context_disconnect(context: *mut opaque::pa_context) {
    assert!(!context.is_null());
    unsafe { ext::pa_context_disconnect(context) };
}

/// A rust wrapper around pa_mainloop_run
pub fn pa_mainloop_run(mainloop: *mut opaque::pa_mainloop, result: &mut c_int) {
    assert!(!mainloop.is_null());
    let res = unsafe{ ext::pa_mainloop_run(mainloop, result as *mut c_int) };
    assert!(res == 0);
}

/// A rust wrapper around pa_context_get_state
pub fn pa_context_get_state(context: *mut opaque::pa_context) -> enums::pa_context_state {
    assert!(!context.is_null());
    unsafe{ ext::pa_context_get_state(context) }
}


/// A rust wrapper around pa_context_get_sink_info_list
pub fn pa_context_get_sink_info_list(context: *mut opaque::pa_context,
    callback: cb::pa_sink_info_cb_t, userdata: *mut c_void) -> Option<*mut opaque::pa_operation> {

    assert!(!context.is_null());
    let result = unsafe{ ext::pa_context_get_sink_info_list(context, callback, userdata) };
    if result.is_null() {
        None
    } else {
        Some(result)
    }
}

/// A rust wrapper around pa_context_get_server_info
pub fn pa_context_get_server_info(context: *mut opaque::pa_context,
    cb: cb::pa_server_info_cb_t, userdata: *mut c_void) -> Option<*mut opaque::pa_operation> {
    assert!(!context.is_null());
    let result = unsafe{ ext::pa_context_get_server_info(context, cb, userdata) };
    if result.is_null() {
        None
    } else {
        Some(result)
    }
}

/// A safe interface to pa_mainloop_get_api
pub fn pa_mainloop_get_api(mainloop: *mut opaque::pa_mainloop) -> *mut opaque::pa_mainloop_api {
    assert!(!mainloop.is_null());
    let mainloop_api = unsafe { ext::pa_mainloop_get_api(mainloop) };
    assert!(!mainloop_api.is_null());
    return mainloop_api;
}

/// A safe interface to pa_context_new
pub fn pa_context_new(mainloop_api: *mut opaque::pa_mainloop_api, client_name: &str) -> *mut opaque::pa_context {
    assert!(!mainloop_api.is_null());
    let client_name_c = CString::from_slice(client_name.as_bytes());
    let context = unsafe{ ext::pa_context_new(mainloop_api, client_name_c.as_ptr()) };
    assert!(!context.is_null());
    return context;
}

/// A safe interface to pa_mainloop_new
pub fn pa_mainloop_new() -> *mut opaque::pa_mainloop {
    let mainloop = unsafe{ ext::pa_mainloop_new() };
    assert!(!mainloop.is_null());
    return mainloop;
}

/// Gets sink info by the sink's name
pub fn pa_context_get_sink_info_by_name(c: *mut pa_context, name: &str, cb: pa_sink_info_cb_t, userdata: *mut c_void) {
    assert!(!c.is_null());
    let name = CString::from_slice(name.as_bytes());
    unsafe{ ext::pa_context_get_sink_info_by_name(c, name.as_ptr(), cb, userdata) };
}


pub fn pa_stream_new(c: *mut opaque::pa_context, name: &str, ss: *const pa_sample_spec, map: *const pa_channel_map) -> *mut opaque::pa_stream {
    assert!(!c.is_null());
    let name = CString::from_slice(name.as_bytes());
    return unsafe { ext::stream::pa_stream_new(c, name.as_ptr(), ss, map) };
}


pub fn pa_stream_peek (
    stream: *mut opaque::pa_stream, data: *const *mut c_void,
    nbytes: *mut size_t) -> c_int {
    assert!(!stream.is_null());
    assert!(!data.is_null());
    assert!(!nbytes.is_null());
    return unsafe { ext::stream::pa_stream_peek(stream, data, nbytes) };
}

pub struct PulseAudioStream {
    pa_stream: *mut opaque::pa_stream
}

impl PulseAudioStream {
    pub fn new(context: *mut pa_context, name: &str, ss: *const pa_sample_spec,
        map: *const pa_channel_map) -> Self {
        PulseAudioStream {
            pa_stream: pa_stream_new(context, name, ss, map)
        }
    }

    /// Reads data into
    pub fn peek(&mut self) -> IoResult<Vec<u8>> {
        let mut buf: *mut u8 = ptr::null_mut();
        let mut nbytes: size_t = 0;

        let mut ret: c_int = 0;
        unsafe {
            ret = pa_stream_peek(self.pa_stream, &(buf as *mut c_void), &mut nbytes);
        };

        if (buf.is_null()) {
            if (nbytes == 0) {
                return Err(IoError {
                    kind: IoErrorKind::NoProgress,
                    desc: "Buffer is empty",
                    detail: None
                });
            } else {
                return Err(IoError {
                    kind: IoErrorKind::OtherIoError,
                    desc: "Hole in input buffer",
                    detail: Some(fmt::format(format_args!("hole of size {}", nbytes))),
                });
            }
        }

        unsafe {
            let data: Vec<u8> = Vec::from_raw_buf(buf, nbytes as usize);
            return Ok(data);
        }
    }
}

/*
impl Reader for PulseAudioStream {
    fn read(&mut self, buf: &mut [u8]) -> IoResult<usize> {
        let mut err: c_int = 0;
        let mut ptr: *mut u8 = buf.as_mut_ptr();
        let mut amount_read:

        // Peek
        unsafe {
            err = ext::stream::pa_stream_peek(
                self.pa_stream, &mut ptr(), buf.len() as size_t);
        }

        if

        // Copy
        {
            let data = slice::from_raw_buf(&ptr, buf.len() as size_t);
        }
        return Err(IoError {
            kind: IoErrorKind::OtherIoError,
            desc: "Not implemented",
            detail: None
        });
    }
}

impl Writer for PulseAudioStream {
    fn write(&mut self, buf: &[u8]) -> IoResult<()> {
        return Err(IoError {
            kind: IoErrorKind::OtherIoError,
            desc: "Not implemented",
            detail: None
        });
    }

}
*/

impl Drop for PulseAudioStream {
    fn drop(&mut self) {
       assert!(!self.pa_stream.is_null());
       println!("[PulseAudioStream] dropped. Disconnecting from pa_stream");
       unsafe { ext::stream::pa_stream_disconnect(self.pa_stream) };
    }
}

mod ext {
    extern crate libc;
    use self::libc::{c_void, c_char, c_int};
    use pulse_types::*;

    #[link(name="pulse")]
    extern {
        pub fn pa_mainloop_new() -> *mut opaque::pa_mainloop;

        pub fn pa_channels_valid(channels: u8) -> c_int;

        pub fn pa_mainloop_get_api(
            m: *mut opaque::pa_mainloop
        ) -> *mut opaque::pa_mainloop_api;

        pub fn pa_context_new(
            mainloop_api: *mut opaque::pa_mainloop_api,
            client_name: *const c_char
        ) -> *mut opaque::pa_context;

        pub fn pa_context_set_state_callback(
            context: *mut opaque::pa_context,
            cb: cb::pa_context_notify_cb_t,
            userdata: *mut c_void
        );

        pub fn pa_context_get_sink_info_by_name(
            c: *mut pa_context,
            name: *const c_char,
            cb: pa_sink_info_cb_t,
            userdata: *mut c_void
        ) -> *mut pa_operation;

        pub fn pa_context_connect(
            context: *mut opaque::pa_context,
            server: *const c_char,
            flags: enums::pa_context_flags,
            api: *const opaque::pa_spawn_api
        ) -> c_int;

        pub fn pa_context_disconnect(context: *mut opaque::pa_context);

        pub fn pa_context_get_state(
            context: *mut opaque::pa_context
        ) -> enums::pa_context_state;

        pub fn pa_mainloop_run(
            m: *mut opaque::pa_mainloop,
            result: *mut c_int
        ) -> c_int;

        pub fn pa_signal_init(
            api: *mut opaque::pa_mainloop_api
        ) -> c_int;


        pub fn pa_proplist_new() -> *mut opaque::pa_proplist;

        pub fn pa_strerror(error: c_int) -> *const c_char;

        pub fn pa_context_get_sink_info_list(
            c: *mut opaque::pa_context,
            cb: cb::pa_sink_info_cb_t,
            userdata: *mut c_void
        ) -> *mut opaque::pa_operation;

        pub fn pa_context_get_server_info(
            c: *mut opaque::pa_context,
            cb: cb::pa_server_info_cb_t,
            userdata: *mut c_void
        ) -> *mut opaque::pa_operation;
    }


    pub mod stream {
        extern crate libc;
        use self::libc::{c_void, c_char, c_int, size_t};
        use pulse_types::*;

        #[link(name="pulse")]
        extern {
            pub fn pa_stream_new(
                c: *mut opaque::pa_context,
                name: *const c_char,
                ss: *const pa_sample_spec,
                map: *const pa_channel_map
           ) -> *mut opaque::pa_stream;

            pub fn pa_stream_new_extended(
                c: *mut opaque::pa_context,
                name: *const c_char,
                formats: *const *const pa_format_info,
                n_formats: c_int,
                p: *mut pa_proplist
            ) -> *mut opaque::pa_stream;

            pub fn pa_stream_set_read_callback(
                p: *mut opaque::pa_stream,
                cb: pa_stream_request_cb_t,
                userdata: *mut c_void);

            pub fn pa_stream_disconnect(s: *mut pa_stream);

            pub fn pa_stream_peek(
                p: *mut pa_stream,
                data: *const *mut c_void,
                nbytes: *mut size_t
            ) -> c_int;

            pub fn pa_stream_drop(p: *mut pa_stream) -> c_int;

        }
    }
}
