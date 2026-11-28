package main

import (
	"fmt"
	"os/user"
)

// os/user goes through the C library (cgo), so this links to glibc.
func main() {
	name := "nobody"
	if u, err := user.Current(); err == nil {
		name = u.Username
	}
	fmt.Printf("go-hello: hello, %s\n", name)
}
