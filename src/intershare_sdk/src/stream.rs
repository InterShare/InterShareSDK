use std::fmt::Debug;
use std::io;
use std::io::{Read, Write};

pub trait Close {
    fn close(&self);
}

#[uniffi::export(callback_interface)]
pub trait NativeStreamDelegate: Send + Sync + Debug {
    fn read(&self, buffer_length: u64) -> Vec<u8>;
    fn write(&self, data: Vec<u8>) -> u64;
    fn flush(&self);
    fn disconnect(&self);
}

impl Read for dyn NativeStreamDelegate {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let data = NativeStreamDelegate::read(self, buf.len() as u64);

        let len = std::cmp::min(buf.len(), data.len());
        buf[..len].copy_from_slice(&data[..len]);

        Ok(len)
    }
}

impl Write for dyn NativeStreamDelegate {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        return Ok(NativeStreamDelegate::write(self, buf.to_vec()) as usize);
    }

    fn flush(&mut self) -> io::Result<()> {
        NativeStreamDelegate::flush(self);

        return Ok(());
    }
}

impl Close for Box<dyn NativeStreamDelegate> {
    fn close(&self) {
        self.disconnect();
    }
}
