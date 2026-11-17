//! Narrow bindings to NVIDIA's public NVAPI I2C interface. No driver is bundled.
//! ABI definitions follow NVIDIA/nvapi nvapi.h (MIT); see third-party notices.
use anyhow::{Context, Result, ensure};
use std::{
    ffi::{CString, c_char, c_void},
    mem::{size_of, transmute_copy},
    ptr::null_mut,
};
use windows_sys::{
    Win32::{
        Foundation::{FreeLibrary, HMODULE},
        System::LibraryLoader::{GetProcAddress, LOAD_LIBRARY_SEARCH_SYSTEM32, LoadLibraryExW},
    },
    core::w,
};

type Handle = *mut c_void;
type Query = unsafe extern "C" fn(u32) -> *const c_void;
type Initialize = unsafe extern "C" fn() -> i32;
type Display = unsafe extern "C" fn(*const c_char, *mut Handle) -> i32;
type Gpus = unsafe extern "C" fn(Handle, *mut Handle, *mut u32) -> i32;
type Output = unsafe extern "C" fn(Handle, *mut u32) -> i32;
type Write = unsafe extern "C" fn(Handle, *mut I2cInfo) -> i32;

#[repr(C)]
struct I2cInfo {
    version: u32,
    display_mask: u32,
    ddc_port: u8,
    address: u8,
    register: *mut u8,
    register_size: u32,
    data: *mut u8,
    size: u32,
    deprecated_speed: u32,
    speed: u32,
    port: u8,
    port_set: u32,
}

struct Api {
    module: HMODULE,
    query: Query,
    unload: Option<Initialize>,
}
impl Api {
    fn load() -> Result<Self> {
        // SAFETY: the DLL name is constant and the search is restricted to the
        // trusted system directory containing the installed display driver.
        let module =
            unsafe { LoadLibraryExW(w!("nvapi64.dll"), null_mut(), LOAD_LIBRARY_SEARCH_SYSTEM32) };
        ensure!(
            !module.is_null(),
            "LG input switching needs the installed NVIDIA NVAPI driver; no input command was sent"
        );
        // SAFETY: `module` is our live loaded library, and the symbol is NUL terminated.
        let query = unsafe { GetProcAddress(module, c"nvapi_QueryInterface".as_ptr().cast()) };
        let Some(query) = query else {
            // SAFETY: the module was loaded above and has no borrowed symbols in use.
            unsafe {
                FreeLibrary(module);
            }
            anyhow::bail!("The NVIDIA driver does not export NVAPI");
        };
        // SAFETY: NVAPI declares this export with exactly the Query C ABI.
        let query: Query = unsafe { std::mem::transmute(query) };
        let mut api = Self {
            module,
            query,
            unload: None,
        };
        // SAFETY: the IDs and signatures are NVIDIA's Initialize/Unload APIs.
        let init: Initialize = unsafe { api.function(0x0150e828)? };
        // SAFETY: this is the SDK's no-argument Unload function.
        let unload: Initialize = unsafe { api.function(0xd22bdd7e)? };
        // SAFETY: NVAPI initialization takes no arguments and changes no display settings.
        ensure!(unsafe { init() } == 0, "NVIDIA NVAPI initialization failed");
        api.unload = Some(unload);
        Ok(api)
    }
    /// # Safety
    /// T must be the exact NVIDIA SDK function-pointer signature for this ID.
    unsafe fn function<T: Copy>(&self, id: u32) -> Result<T> {
        // SAFETY: the query export has the SDK ABI and the DLL remains loaded.
        let pointer = unsafe { (self.query)(id) };
        ensure!(
            !pointer.is_null() && size_of::<T>() == size_of::<*const c_void>(),
            "The NVIDIA driver does not support required NVAPI function 0x{id:08x}"
        );
        // SAFETY: the caller supplies the documented signature, validated as a
        // non-null function pointer of the same size; the DLL outlives the result.
        Ok(unsafe { transmute_copy(&pointer) })
    }
}
impl Drop for Api {
    fn drop(&mut self) {
        if let Some(unload) = self.unload {
            // SAFETY: paired with this instance's successful Initialize, after
            // all of its synchronous operations and borrowed GPU handles finish.
            unsafe {
                unload();
            }
        }
        // SAFETY: this guard owns the library reference and no symbols escape it.
        unsafe {
            FreeLibrary(self.module);
        }
    }
}

pub(super) struct Prepared {
    // Own the library/initialization for every borrowed function and GPU handle.
    _api: Api,
    gpu: Handle,
    output: u32,
    write: Write,
}
pub(super) fn prepare(gdi: &str) -> Result<Prepared> {
    let api = Api::load()?;
    let name = CString::new(gdi).context("Invalid Windows monitor name")?;
    // SAFETY: each ID is paired with its exact public NVIDIA SDK signature.
    let display: Display = unsafe { api.function(0x35c29134)? };
    // SAFETY: this is the SDK's GetPhysicalGPUsFromDisplay function.
    let gpus: Gpus = unsafe { api.function(0x34ef9506)? };
    // SAFETY: this is the SDK's GetAssociatedDisplayOutputId function.
    let output: Output = unsafe { api.function(0xd995937e)? };
    // SAFETY: this is the SDK's I2CWrite function, resolved but not called here.
    let write: Write = unsafe { api.function(0xe812eb07)? };
    let mut handle = null_mut();
    let mut gpu = [null_mut(); 64];
    let mut count = 0;
    let mut mask = 0;
    // SAFETY: name is NUL terminated, all output buffers are writable and live.
    let display_status = unsafe { display(name.as_ptr(), &mut handle) };
    ensure!(
        display_status == 0 && !handle.is_null(),
        "The selected monitor is not driven by NVIDIA"
    );
    // SAFETY: the display handle is live and the array holds the SDK's maximum
    // 64 physical GPU handles. Require one unambiguous physical GPU below.
    let gpu_status = unsafe { gpus(handle, gpu.as_mut_ptr(), &mut count) };
    ensure!(
        gpu_status == 0 && count == 1 && !gpu[0].is_null(),
        "Cannot unambiguously identify the selected monitor's NVIDIA GPU"
    );
    // SAFETY: the handle identifies that same display and mask is a live output.
    let output_status = unsafe { output(handle, &mut mask) };
    ensure!(
        output_status == 0 && mask.count_ones() == 1,
        "The selected NVIDIA monitor output is ambiguous"
    );
    Ok(Prepared {
        _api: api,
        gpu: gpu[0],
        output: mask,
        write,
    })
}
impl Prepared {
    pub(super) fn write_input(&self, value: u8) -> Result<()> {
        ensure!(
            matches!(value, 0x11 | 0x12 | 0x0f),
            "Unsupported LG input value"
        );
        let mut source = 0x51;
        let mut packet = crate::video::lg::packet(value);
        let mut info = I2cInfo {
            version: size_of::<I2cInfo>() as u32 | (3 << 16),
            display_mask: self.output,
            ddc_port: 1,
            address: 0x6e,
            register: &mut source,
            register_size: 1,
            data: packet.as_mut_ptr(),
            size: packet.len() as u32,
            deprecated_speed: 0xffff,
            speed: 0,
            port: 0,
            port_set: 0,
        };
        println!(
            "LG input write: NVIDIA output {:#x}, source 0x51, VCP 0x60, value {value:#04x}; one I2C command",
            self.output
        );
        // SAFETY: the selected GPU/output and all buffers stay live through
        // exactly one synchronous SDK call. This is the same transport and
        // standard packet verified on the physical 27GK750F, without retries.
        let status = unsafe { (self.write)(self.gpu, &mut info) };
        ensure!(
            status == 0,
            "NVIDIA standard input command failed with status {status}; no retry"
        );
        Ok(())
    }
    #[cfg(test)]
    pub(super) fn inspect_input_read(&self, source: u8, feature: u8) -> Result<Vec<u8>> {
        ensure!(
            matches!((source, feature), (0x51, 0x60) | (0x50, 0xf4)),
            "Only the two named input-feature reads are supported"
        );
        // SAFETY: this ID is the public SDK's I2CRead with the same I2C_INFO ABI.
        let read: Write = unsafe { self._api.function(0x2fde12c5)? };
        let mut source = source;
        // Opcode 0x01 requests a value. It never selects an input (opcode 0x03).
        let mut query = [0x82, 0x01, feature, 0x6e ^ source ^ 0x82 ^ 0x01 ^ feature];
        let mut info = I2cInfo {
            version: size_of::<I2cInfo>() as u32 | (3 << 16),
            display_mask: self.output,
            ddc_port: 1,
            address: 0x6e,
            register: &mut source,
            register_size: 1,
            data: query.as_mut_ptr(),
            size: query.len() as u32,
            deprecated_speed: 0xffff,
            speed: 0,
            port: 0,
            port_set: 0,
        };
        // SAFETY: all owned buffers and this exact selected output stay live;
        // the packet requests a value and contains no input-selection opcode.
        let status = unsafe { (self.write)(self.gpu, &mut info) };
        ensure!(
            status == 0,
            "NVIDIA input query failed with status {status}"
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
        let mut reply = vec![0; 11];
        info.address = 0x6f;
        info.register = null_mut();
        info.register_size = 0;
        info.data = reply.as_mut_ptr();
        info.size = reply.len() as u32;
        // SAFETY: I2CRead writes only into the live reply buffer; the API and
        // selected GPU/output outlive the synchronous read.
        let status = unsafe { read(self.gpu, &mut info) };
        ensure!(
            status == 0,
            "NVIDIA input reply failed with status {status}"
        );
        Ok(reply)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn i2c_info_matches_the_nvidia_x64_abi() {
        assert_eq!(size_of::<I2cInfo>(), 64);
        assert_eq!(std::mem::offset_of!(I2cInfo, register), 16);
        assert_eq!(std::mem::offset_of!(I2cInfo, data), 32);
        assert_eq!(std::mem::offset_of!(I2cInfo, port_set), 56);
    }
}
