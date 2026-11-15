//! No-replace Windows publication from the approved open file handle.
//!
//! Sharing flags alone do not establish the identity of a pathname for every
//! native rename/link operation. `FileLinkInformation` binds the source handle
//! and destination directory handle without reopening the staging name.
//! Native layout: <https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_link_information>.
//! Synchronous I/O: <https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/nf-ntifs-ntcreatefile>.

#![allow(unsafe_code)]

use std::io;
use std::mem::{align_of, offset_of, size_of};
use std::os::windows::io::AsRawHandle;

use windows::Wdk::Storage::FileSystem::{
    FILE_LINK_INFORMATION, FileLinkInformation, NtSetInformationFile,
};
use windows::Win32::Foundation::{HANDLE, RtlNtStatusToDosError, STATUS_SUCCESS};
use windows::Win32::System::IO::IO_STATUS_BLOCK;

use super::StagedBrainNote;

/// The native flexible array is inlined: appending to a generated header would
/// put its tail padding between the first and subsequent filename code units.
#[repr(C)]
struct LinkBuffer {
    flags: u32,
    directory: HANDLE,
    filename_bytes: u32,
    filename: [u16; 100],
}

const _: () = {
    assert!(offset_of!(LinkBuffer, directory) == offset_of!(FILE_LINK_INFORMATION, RootDirectory));
    assert!(
        offset_of!(LinkBuffer, filename_bytes) == offset_of!(FILE_LINK_INFORMATION, FileNameLength)
    );
    assert!(offset_of!(LinkBuffer, filename) == offset_of!(FILE_LINK_INFORMATION, FileName));
    assert!(align_of::<LinkBuffer>() >= align_of::<FILE_LINK_INFORMATION>());
};

impl LinkBuffer {
    fn new(directory: HANDLE, filename: &str) -> io::Result<Self> {
        if filename.is_empty()
            || filename.len() > 100
            || matches!(filename, "." | "..")
            || !filename
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-' | b'_'))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "brain publication requires a bounded local filename",
            ));
        }
        let mut buffer = Self {
            // Legacy FileLinkInformation reads ReplaceIfExists from this
            // union. Zero prohibits replacement, including existing symlinks.
            flags: 0,
            directory,
            filename_bytes: u32::try_from(filename.len() * 2).expect("filename is bounded"),
            filename: [0; 100],
        };
        for (destination, byte) in buffer.filename.iter_mut().zip(filename.bytes()) {
            *destination = u16::from(byte);
        }
        Ok(buffer)
    }

    fn byte_len(&self) -> u32 {
        let used = offset_of!(Self, filename)
            + usize::try_from(self.filename_bytes).expect("filename is bounded");
        u32::try_from(used.max(size_of::<FILE_LINK_INFORMATION>()))
            .expect("native buffer is bounded")
    }
}

pub(super) fn publish(staging: &StagedBrainNote, filename: &str) -> io::Result<()> {
    let source = staging
        .file
        .as_ref()
        .expect("staging stays open until drop");
    let buffer = LinkBuffer::new(HANDLE(staging.directory.as_raw_handle()), filename)?;
    let mut status_block = IO_STATUS_BLOCK::default();
    // SAFETY: the private StagedBrainNote constructor opens a synchronous file
    // (no FILE_FLAG_OVERLAPPED). The source and directory are borrowed for the
    // whole call, so neither handle can close. LinkBuffer is aligned, its field
    // offsets are checked against the SDK, and byte_len never exceeds it. The
    // bounded filename contains no separators, NUL or drive prefix. Both stack
    // buffers stay alive through synchronous completion. Flags=0 selects
    // no-replace publication; the kernel never resolves a source pathname.
    let status = unsafe {
        NtSetInformationFile(
            HANDLE(source.as_raw_handle()),
            &raw mut status_block,
            std::ptr::from_ref(&buffer).cast(),
            buffer.byte_len(),
            FileLinkInformation,
        )
    };
    if status != STATUS_SUCCESS {
        // SAFETY: this pure status conversion takes no pointers or ownership.
        let error = unsafe { RtlNtStatusToDosError(status) };
        return Err(io::Error::from_raw_os_error(
            i32::try_from(error).unwrap_or(i32::MAX),
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use cap_std::fs::{File, OpenOptions, OpenOptionsExt};
    use protocol::{BotId, UserId};
    use uuid::Uuid;
    use windows::Wdk::Storage::FileSystem::{
        FILE_INFORMATION_CLASS, FILE_LINK_POSIX_SEMANTICS, FILE_LINK_REPLACE_IF_EXISTS,
        FILE_RENAME_INFORMATION, FileLinkInformationEx, FileRenameInformationEx,
    };
    use windows::Win32::Foundation::{
        NTSTATUS, STATUS_ACCESS_DENIED, STATUS_CANNOT_DELETE, STATUS_SHARING_VIOLATION,
    };
    use windows::Win32::Storage::FileSystem::{
        DELETE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ,
        FILE_SHARE_WRITE,
    };

    use super::super::{BrainWriteInput, PreparedBrainWrite};
    use super::*;
    use crate::knowledge::{KnowledgeStore, Scope};

    const _: () = {
        assert!(
            offset_of!(LinkBuffer, directory) == offset_of!(FILE_RENAME_INFORMATION, RootDirectory)
        );
        assert!(
            offset_of!(LinkBuffer, filename_bytes)
                == offset_of!(FILE_RENAME_INFORMATION, FileNameLength)
        );
        assert!(offset_of!(LinkBuffer, filename) == offset_of!(FILE_RENAME_INFORMATION, FileName));
        assert!(align_of::<LinkBuffer>() >= align_of::<FILE_RENAME_INFORMATION>());
    };

    fn replace_native(
        source: &File,
        staging: &StagedBrainNote,
        filename: &str,
        class: FILE_INFORMATION_CLASS,
    ) -> NTSTATUS {
        let mut buffer =
            LinkBuffer::new(HANDLE(staging.directory.as_raw_handle()), filename).unwrap();
        buffer.flags = FILE_LINK_REPLACE_IF_EXISTS | FILE_LINK_POSIX_SEMANTICS;
        let mut status_block = IO_STATUS_BLOCK::default();
        // SAFETY: tests open source synchronously below and keep both handles
        // alive. The bounded buffer has verified link/rename SDK layout. Only
        // these temporary-file tests pass the Ex replacement flags/classes.
        unsafe {
            NtSetInformationFile(
                HANDLE(source.as_raw_handle()),
                &raw mut status_block,
                std::ptr::from_ref(&buffer).cast(),
                buffer.byte_len(),
                class,
            )
        }
    }

    fn native_replacement_preserves_approved_publication(class: FILE_INFORMATION_CLASS) {
        let data = tempfile::tempdir().unwrap();
        let scope = Scope {
            user_id: UserId::new(),
            bot_id: BotId::new(),
        };
        let store = KnowledgeStore::open_in_memory(scope).unwrap();
        let prepared = PreparedBrainWrite::prepare(
            data.path(),
            &store,
            scope,
            Uuid::now_v7(),
            &BrainWriteInput {
                filename: "decision.md".into(),
                content: "Only these approved bytes may be published.".into(),
                sources: vec![],
            },
        )
        .unwrap();
        let approved = prepared.preview().content.clone();
        let staging = prepared.stage().unwrap();
        let mut options = OpenOptions::new();
        options
            .read(true)
            .write(true)
            .create_new(true)
            .access_mode((FILE_GENERIC_READ | FILE_GENERIC_WRITE | DELETE).0)
            .share_mode((FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE).0);

        // Positive control proves the Ex operation and flags actually replace
        // an open destination on this filesystem. Unsupported or invalid calls
        // cannot be mistaken for a successful protection test.
        let mut control = staging
            .directory
            .open_with("control.tmp", &options)
            .unwrap();
        control.write_all(b"ORIGINAL-CONTROL").unwrap();
        let mut source = staging.directory.open_with("source.tmp", &options).unwrap();
        source.write_all(b"UNAPPROVED-REPLACEMENT").unwrap();
        source.sync_all().unwrap();
        assert_eq!(
            replace_native(&source, &staging, "control.tmp", class),
            STATUS_SUCCESS,
            "native POSIX replacement positive control must succeed"
        );
        assert_eq!(
            staging.directory.read("control.tmp").unwrap(),
            b"UNAPPROVED-REPLACEMENT"
        );

        // Use a fresh source because a rename changed source's original link.
        let mut attacker = staging
            .directory
            .open_with("attacker.tmp", &options)
            .unwrap();
        attacker.write_all(b"UNAPPROVED-ATTACK").unwrap();
        attacker.sync_all().unwrap();
        let status = replace_native(&attacker, &staging, staging.name.as_ref().unwrap(), class);
        assert!(
            matches!(
                status,
                STATUS_SUCCESS
                    | STATUS_ACCESS_DENIED
                    | STATUS_SHARING_VIOLATION
                    | STATUS_CANNOT_DELETE
            ),
            "unexpected native replacement status: {status:?}"
        );
        let outcome = prepared.publish(&store, &staging);
        if status != STATUS_SUCCESS {
            outcome
                .as_ref()
                .expect("a blocked replacement leaves a usable stage");
        }
        match outcome {
            Ok(_) => assert_eq!(
                std::fs::read_to_string(data.path().join("brain/decision.md")).unwrap(),
                approved
            ),
            // Windows may refuse to link an original handle whose pathname
            // was unlinked. A safe failure must not create any final note.
            Err(_) => assert!(!data.path().join("brain/decision.md").exists()),
        }
    }

    #[test]
    fn native_posix_link_cannot_substitute_approved_bytes() {
        native_replacement_preserves_approved_publication(FileLinkInformationEx);
    }

    #[test]
    fn native_posix_rename_cannot_substitute_approved_bytes() {
        native_replacement_preserves_approved_publication(FileRenameInformationEx);
    }
}
