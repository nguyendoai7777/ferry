//! Một daemon mỗi người dùng: khoá độc quyền tệp `<data_dir>/daemon.lock` suốt đời tiến trình (bê từ fastcast). OS tự nhả
//! khoá khi tiến trình chết (kể cả crash) nên không có khoá treo; trên Unix khoá còn bảo vệ việc xoá tệp socket cũ.

use std::fs::{self, File, TryLockError};
use std::io::{self, Write};
use std::path::Path;

pub struct InstanceLock {
    _file: File,
}

/// `Ok(None)` ⇒ đã có daemon khác giữ khoá.
pub fn acquire(dir: &Path) -> io::Result<Option<InstanceLock>> {
    fs::create_dir_all(dir)?;
    let mut file = File::options().create(true).truncate(false).read(true).write(true).open(dir.join("daemon.lock"))?;
    match file.try_lock() {
        Ok(()) => {
            // PID chỉ để người đọc tệp biết; không dùng để quyết định gì.
            file.set_len(0)?;
            write!(file, "{}", std::process::id())?;
            Ok(Some(InstanceLock { _file: file }))
        }
        Err(TryLockError::WouldBlock) => Ok(None),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn second_acquire_fails_until_first_dropped() {
        let dir = tempfile::tempdir().unwrap();
        let first = acquire(dir.path()).unwrap().expect("first lock");
        assert!(acquire(dir.path()).unwrap().is_none());
        drop(first);
        assert!(acquire(dir.path()).unwrap().is_some());
    }
}
