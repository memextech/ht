use std::io::Write;
use std::io::{self, ErrorKind, Read};

#[cfg(unix)]
use std::os::fd::RawFd;

#[cfg(unix)]
pub fn set_non_blocking(fd: &RawFd) -> Result<(), io::Error> {
    use nix::fcntl::{fcntl, FcntlArg::*, OFlag};

    let flags = fcntl(*fd, F_GETFL)?;
    let mut oflags = OFlag::from_bits_truncate(flags);
    oflags |= OFlag::O_NONBLOCK;
    fcntl(*fd, F_SETFL(oflags))?;

    Ok(())
}

#[cfg(windows)]
pub fn set_non_blocking(_handle: &windows::Win32::Foundation::HANDLE) -> Result<(), io::Error> {
    // On Windows, we handle non-blocking I/O differently
    // This is a placeholder as winpty handles this for us
    Ok(())
}

pub fn read<R: Read + ?Sized>(source: &mut R, buf: &mut [u8]) -> io::Result<Option<usize>> {
    match source.read(buf) {
        Ok(n) => Ok(Some(n)),

        Err(e) => {
            if e.kind() == ErrorKind::WouldBlock {
                Ok(None)
            } else if e.raw_os_error().is_some_and(|code| code == 5) {
                Ok(Some(0))
            } else {
                return Err(e);
            }
        }
    }
}

pub fn write<W: Write + ?Sized>(sink: &mut W, buf: &[u8]) -> io::Result<Option<usize>> {
    match sink.write(buf) {
        Ok(n) => Ok(Some(n)),

        Err(e) => {
            if e.kind() == ErrorKind::WouldBlock {
                Ok(None)
            } else if e.raw_os_error().is_some_and(|code| code == 5) {
                Ok(Some(0))
            } else {
                return Err(e);
            }
        }
    }
}
