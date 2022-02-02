# Barebone boot sectors

The examples in this folder are showing you how you can create a barebone boot sector without any dependencies.

* `01_bootsector.asm`
    * A traditional, famous "Hello World" boot sector
* `02_bootsector_printline.asm`
    * This boot sector shows how to setup the stack and how to call functions

You can build the barebone bootsectors from this folder with the following command - just pass in the necessary bootsector assembly file as an argument:

```shell
docker run --rm -it -v $HOME/Documents/GitHub/SQLpassion/osdev:/src sqlpassion/kaos-buildenv
    /bin/sh /src/tutorials/001-bootsectors-barebones/build.sh 01_bootsector.asm
```