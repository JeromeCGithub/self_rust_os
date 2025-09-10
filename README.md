# self_rust_os
A self made simple OS in Rust.

This is an experimental repository to learn how to make an OS in Rust.

This project is based on the blog post series [Writing an OS in Rust](https://os.phil-opp.com/). I recommend you to read it if you want to learn how to make an OS in Rust.

I'm following the blog post of Philipp Oppermann, then trying to enhance the OS with more features that make me learn more about OS development.

## Features

- [x] VGA text buffer
- [ ] Interrupts
- [ ] Memory management
- [ ] Multitasking
- [ ] File system
- [ ] User mode

## Requirements

- [QEMU](https://www.qemu.org/)
- [Rust](https://www.rust-lang.org/)

You will need to install these dependencies :

``` bash
$ cargo install bootimage
$ rustup component add llvm-tools-preview
```

## How to run

```bash
$ cargo run
```

## Contributing

If like me you want to learn more about OS development in rust, feel free to contribute to this project. You can open an issue or a pull request. I will be happy to discuss with you about this project and its implementation.

## License  

This project is licensed under the MIT License - see the [LICENSE-MIT](LICENSE-MIT) file for details.

This project is also licensed under the Apache 2.0 License - see the [LICENSE-APACHE](LICENSE-APACHE) file for details.
