# Folio's Nautilus plugin. Nautilus 50 loads every shared library in
# <libdir>/nautilus/extensions-4 and nothing else. Build against the system GTK 4.
CC ?= gcc
CFLAGS ?= -O2 -Wall -Wextra -Wno-missing-field-initializers
PKG_CONFIG ?= pkg-config

libfolio-files.so: folio-files.c
	$(CC) -shared -fPIC $(CFLAGS) -o $@ folio-files.c \
		$$($(PKG_CONFIG) --cflags --libs gtk4) -ldl

clean:
	rm -f libfolio-files.so

.PHONY: clean
