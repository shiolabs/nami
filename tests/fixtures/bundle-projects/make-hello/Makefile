PREFIX ?= /usr/local

make-hello: hello.c
	$(CC) -O2 -o $@ hello.c

install: make-hello
	install -Dm755 make-hello $(DESTDIR)$(PREFIX)/bin/make-hello
