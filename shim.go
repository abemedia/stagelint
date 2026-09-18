package main

import "github.com/abemedia/go-shim"

func main() {
	shim.Main(shim.Config{
		URL:     "{{.Name}}-{{.Version}}-{{.Target}}{{.Ext}}",
		Targets: shim.Rust,
	})
}
