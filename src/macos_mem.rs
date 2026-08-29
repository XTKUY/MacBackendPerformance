//! macOS 内存统计：按活动监视器同款口径计算（App + Wired + Compressed），
//! 并附带压缩内存、inactive、free、swap 总量等明细。

#[derive(Debug, Clone, Copy, Default)]
pub struct MemStats {
    pub total_bytes: u64,
    /// 活动监视器口径：App Memory + Wired + Compressed
    pub used_bytes: u64,
    pub app_bytes: u64,
    pub wired_bytes: u64,
    pub compressed_bytes: u64,
    pub inactive_bytes: u64,
    pub free_bytes: u64,
    pub swap_total_bytes: u64,
    pub swap_used_bytes: u64,
}

#[cfg(target_os = "macos")]
#[allow(non_camel_case_types)]
mod imp {
    use super::MemStats;
    use std::mem;
    use std::os::raw::{c_int, c_uint, c_void};

    const CTL_HW: c_int = 6;
    const HW_MEMSIZE: c_int = 24;
    const CTL_VM: c_int = 2;
    const VM_SWAPUSAGE: c_int = 5;
    const HOST_VM_INFO64: c_uint = 4;
    const KERN_SUCCESS: c_int = 0;
    const _SC_PAGESIZE: c_int = 29;

    type mach_port_t = c_uint;
    type natural_t = c_uint;
    type mach_msg_type_number_t = c_uint;
    type kern_return_t = c_int;

    extern "C" {
        fn mach_host_self() -> mach_port_t;
        fn host_statistics64(
            host: mach_port_t,
            flavor: c_uint,
            info: *mut natural_t,
            count: *mut mach_msg_type_number_t,
        ) -> kern_return_t;
    }

    #[repr(C)]
    #[derive(Debug, Default, Clone, Copy)]
    struct VmStatistics64 {
        free_count: u32,
        active_count: u32,
        inactive_count: u32,
        wire_count: u32,
        zero_fill_count: u64,
        reactivations: u64,
        pageins: u64,
        pageouts: u64,
        faults: u64,
        cow_faults: u64,
        lookups: u64,
        hits: u64,
        purges: u64,
        purgeable_count: u32,
        speculative_count: u32,
        decompressions: u64,
        compressions: u64,
        swapins: u64,
        swapouts: u64,
        compressor_page_count: u32,
        throttled_count: u32,
        external_page_count: u32,
        internal_page_count: u32,
        total_uncompressed_pages_in_compressor: u64,
    }

    #[repr(C)]
    #[derive(Debug, Default, Clone, Copy)]
    struct XswUsage {
        xsu_total: u64,
        xsu_avail: u64,
        xsu_used: u64,
        xsu_pagesize: u32,
        xsu_encrypted: i32,
    }

    fn page_size() -> u64 {
        let p = unsafe { libc::sysconf(_SC_PAGESIZE) };
        if p > 0 {
            p as u64
        } else {
            16_384
        }
    }

    fn sysctl_value<T: Copy + Default>(mib: &[c_int]) -> Option<T> {
        let mut val: T = Default::default();
        let mut size = mem::size_of::<T>();
        let ret = unsafe {
            libc::sysctl(
                mib.as_ptr() as *mut c_int,
                mib.len() as c_uint,
                &mut val as *mut T as *mut c_void,
                &mut size,
                std::ptr::null_mut(),
                0,
            )
        };
        if ret == 0 {
            Some(val)
        } else {
            None
        }
    }

    pub fn read() -> Option<MemStats> {
        let page = page_size();
        let total_bytes = sysctl_value::<u64>(&[CTL_HW, HW_MEMSIZE])?;

        let mut stat: VmStatistics64 = Default::default();
        let mut count = (mem::size_of::<VmStatistics64>() / mem::size_of::<natural_t>()) as u32;
        let ret = unsafe {
            host_statistics64(
                mach_host_self(),
                HOST_VM_INFO64,
                &mut stat as *mut VmStatistics64 as *mut natural_t,
                &mut count,
            )
        };
        if ret != KERN_SUCCESS {
            return None;
        }

        let mult = |n: u32| -> u64 { u64::from(n) * page };
        let free_count = stat.free_count.saturating_sub(stat.speculative_count);
        let app = (stat
            .internal_page_count
            .saturating_sub(stat.purgeable_count)) as u64
            * page;
        let wired = mult(stat.wire_count);
        let compressed = mult(stat.compressor_page_count);

        let xs: Option<XswUsage> = sysctl_value(&[CTL_VM, VM_SWAPUSAGE]);

        Some(MemStats {
            total_bytes,
            used_bytes: app + wired + compressed,
            app_bytes: app,
            wired_bytes: wired,
            compressed_bytes: compressed,
            inactive_bytes: mult(stat.inactive_count),
            free_bytes: mult(free_count),
            swap_total_bytes: xs.map(|x| x.xsu_total).unwrap_or(0),
            swap_used_bytes: xs.map(|x| x.xsu_used).unwrap_or(0),
        })
    }
}

#[cfg(not(target_os = "macos"))]
mod imp {
    use super::MemStats;

    pub fn read() -> Option<MemStats> {
        None
    }
}

pub use imp::read;

#[cfg(test)]
mod tests {
    #[cfg(target_os = "macos")]
    #[test]
    fn macos_mem_stats_are_sane() {
        let m = super::read().expect("macOS 上应能读取内存统计");
        assert!(m.total_bytes > 0);
        assert!(m.used_bytes <= m.total_bytes);
        assert!(m.app_bytes + m.wired_bytes + m.compressed_bytes == m.used_bytes);
    }
}
