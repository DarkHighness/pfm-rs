use super::util::pfm_err_description;
use libc::{c_int, c_longlong, c_ulong, c_void, pid_t, read};
use perf_event_open_sys::bindings::PERF_FLAG_FD_CLOEXEC;
use perf_event_open_sys::{ioctls, perf_event_open};
use pfm_sys::{
    perf_event_attr, perf_event_read_format, pfm_get_os_event_encoding, pfm_os_t,
    pfm_perf_encode_arg_t, PFM_PLM3, PFM_SUCCESS,
};
use std::ffi::CString;
use std::fmt::Display;
use std::mem::MaybeUninit;

#[derive(Clone)]
pub struct PerfEvent {
    name: String,
    attrs: perf_event_attr,
    fd: Option<c_int>,
}

#[derive(Copy, Clone, Debug)]
pub struct PerfEventValue {
    pub value: i64,
    pub time_enabled: i64,
    pub time_running: i64,
}

impl PerfEvent {
    pub fn new(name: &str, inherit: bool) -> Result<Self, String> {
        let cstring = CString::new(name).expect("Event name should be a valid C string");
        let mut attrs: perf_event_attr = unsafe { MaybeUninit::zeroed().assume_init() };
        let mut pfm_args: pfm_perf_encode_arg_t = unsafe { MaybeUninit::zeroed().assume_init() };
        pfm_args.size = std::mem::size_of::<pfm_perf_encode_arg_t>();
        pfm_args.attr = &mut attrs;
        let errno = unsafe {
            pfm_get_os_event_encoding(
                cstring.as_ptr(),
                PFM_PLM3,
                // modifiers exported by the underlying PMU hardware
                // +  modifiers controlled only by the perf_event interface,
                // such as sampling period (period), frequency (freq) and
                // exclusive resource access (excl).
                pfm_os_t::PFM_OS_PERF_EVENT_EXT,
                &mut pfm_args as *mut pfm_perf_encode_arg_t as *mut c_void,
            )
        };
        if errno != PFM_SUCCESS {
            return Err(pfm_err_description(errno));
        }
        assert_eq!(attrs.freq(), 0); // we must be using period for the above union field access
        attrs.read_format = (perf_event_read_format::PERF_FORMAT_TOTAL_TIME_ENABLED as u64)
            | (perf_event_read_format::PERF_FORMAT_TOTAL_TIME_RUNNING as u64);
        attrs.set_disabled(1);
        if inherit {
            attrs.set_inherit(1);
        }

        Ok(Self {
            name: name.to_string(),
            attrs,
            fd: None,
        })
    }

    pub fn set_exclude_kernel(&mut self, val: u64) {
        self.attrs.set_exclude_kernel(val)
    }

    pub fn set_freq(&mut self, val: u64) {
        self.attrs.set_freq(1);
        self.attrs.__bindgen_anon_1.sample_freq = val;
    }

    pub fn set_period(&mut self, val: u64) {
        self.attrs.set_freq(0);
        self.attrs.__bindgen_anon_1.sample_period = val;
    }

    pub fn open(
        &mut self,
        pid: pid_t,
        cpu: c_int,
        group_fd: c_int,
        flags: Option<c_ulong>,
    ) -> Result<(), std::io::Error> {
        let flags = flags.unwrap_or(PERF_FLAG_FD_CLOEXEC as u64);
        let flags = flags | PERF_FLAG_FD_CLOEXEC as u64;

        let result = unsafe {
            perf_event_open(
                &mut self.attrs as *mut pfm_sys::perf_event_attr
                    as *mut perf_event_open_sys::bindings::perf_event_attr,
                pid,
                cpu,
                group_fd,
                flags,
            )
        };
        if result == -1 {
            Err(std::io::Error::last_os_error())
        } else {
            self.fd = Some(result);
            Ok(())
        }
    }

    pub fn enable(&mut self) -> Result<(), std::io::Error> {
        if let Some(fd) = self.fd {
            let result = unsafe { ioctls::ENABLE(fd, 0) };
            if result == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        } else {
            panic!("Enabing a perf event that hasn't been opened");
        }
    }

    pub fn disable(&mut self) -> Result<(), std::io::Error> {
        if let Some(fd) = self.fd {
            let result = unsafe { ioctls::DISABLE(fd, 0) };
            if result == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        } else {
            panic!("Disabling a perf event that hasn't been opened");
        }
    }

    pub fn reset(&mut self) -> Result<(), std::io::Error> {
        if let Some(fd) = self.fd {
            let result = unsafe { ioctls::RESET(fd, 0) };
            if result == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(())
            }
        } else {
            panic!("Resetting a perf event that hasn't been opened");
        }
    }

    pub fn read(&self) -> Result<PerfEventValue, std::io::Error> {
        let mut counts: [c_longlong; 3] = [0; 3];
        if let Some(fd) = self.fd {
            let result = unsafe {
                read(
                    fd,
                    (&mut counts) as *mut _ as *mut c_void,
                    std::mem::size_of::<[c_longlong; 3]>(),
                )
            };
            if result == -1 {
                Err(std::io::Error::last_os_error())
            } else {
                Ok(PerfEventValue {
                    value: counts[0],
                    time_enabled: counts[1],
                    time_running: counts[2],
                })
            }
        } else {
            panic!("Reading a perf event that hasn't been opened");
        }
    }
}

impl Display for PerfEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "PERF[type={} config=0x{:x} config1=0x{:x} \
        excl={:?} excl_user={} excl_kernel={} excl_hv={} excl_host={} \
        excl_guest={} period={} freq={} precise={} pinned={}] {}",
            self.attrs.type_,
            self.attrs.config,
            // the union of u64 bp_addr/kprobe_func/uprobe_path/config1
            unsafe { self.attrs.__bindgen_anon_3.config1 },
            self.attrs.exclusive(),
            self.attrs.exclude_user(),
            self.attrs.exclude_kernel(),
            self.attrs.exclude_hv(),
            self.attrs.exclude_host(),
            self.attrs.exclude_guest(),
            // the union of u64 sample_period/sample_freq
            unsafe { self.attrs.__bindgen_anon_1.sample_period },
            self.attrs.freq(),
            self.attrs.precise_ip(),
            self.attrs.pinned(),
            self.name
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_instructions() {
        use crate::Perfmon;
        let mut perfmon: Perfmon = Default::default();
        perfmon.initialize().unwrap();
        let mut event = PerfEvent::new("INST_RETIRED.ANY_P", false).unwrap();
        event.open(0, -1, -1, None).unwrap();
        event.reset().unwrap();
        event.enable().unwrap();
        println!("Measuring instruction count for this println");
        let counts = event.read().unwrap();
        println!(
            "Raw: {}, total time enabled: {}, total time running {}",
            counts.value, counts.time_enabled, counts.time_running
        );
        event.disable().unwrap();
    }

    #[test]
    fn test_ref_distributed() {
        use crate::Perfmon;
        let mut perfmon: Perfmon = Default::default();
        perfmon.initialize().unwrap();
        let mut event = PerfEvent::new("CPU_CLK_UNHALTED.REF_DISTRIBUTED", false).unwrap();
        event.open(-1, 0, -1, None).unwrap();
        event.reset().unwrap();
        event.enable().unwrap();
        println!("Measuring instruction count for this println");
        let counts = event.read().unwrap();
        println!(
            "Raw: {}, total time enabled: {}, total time running {}",
            counts.value, counts.time_enabled, counts.time_running
        );
        event.disable().unwrap();
    }
}
