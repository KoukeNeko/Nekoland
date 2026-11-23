package main

import (
	"bytes"
	"io"
	"os"
	"syscall"
	"testing"
	"unsafe"

	"golang.org/x/sys/unix"
)

// openPTY opens a pseudo terminal. What the program under test writes to the
// slave side can be read from the master side, and so can the echo of what is
// typed, if the slave side echoes it.
func openPTY(t *testing.T) (master *os.File, slave *os.File, output *syncBuffer) {
	t.Helper()
	master, err := os.OpenFile("/dev/ptmx", os.O_RDWR|syscall.O_NOCTTY, 0)
	if err != nil {
		t.Skipf("no pseudo terminal: %v", err)
	}
	t.Cleanup(func() { master.Close() })
	fd := uintptr(master.Fd())
	if err := unix.IoctlSetInt(int(fd), unix.TIOCPTYGRANT, 0); err != nil {
		t.Fatal(err)
	}
	if err := unix.IoctlSetInt(int(fd), unix.TIOCPTYUNLK, 0); err != nil {
		t.Fatal(err)
	}
	var name [128]byte
	if _, _, errno := unix.Syscall(unix.SYS_IOCTL, fd, unix.TIOCPTYGNAME, uintptr(unsafe.Pointer(&name[0]))); errno != 0 {
		t.Fatal(errno)
	}
	slave, err = os.OpenFile(string(name[:bytes.IndexByte(name[:], 0)]), os.O_RDWR|syscall.O_NOCTTY, 0)
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { slave.Close() })

	output = &syncBuffer{}
	go io.Copy(output, master)
	return master, slave, output
}

func echoes(t *testing.T, f *os.File) bool {
	t.Helper()
	termios, err := unix.IoctlGetTermios(int(f.Fd()), unix.TIOCGETA)
	if err != nil {
		t.Fatal(err)
	}
	return termios.Lflag&unix.ECHO != 0
}
