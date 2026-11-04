packer {
  required_version = ">= 1.16.1"

  required_plugins {
    virtualbox = {
      version = "= 1.1.5"
      source  = "github.com/hashicorp/virtualbox"
    }
    vagrant = {
      version = "= 1.1.7"
      source  = "github.com/hashicorp/vagrant"
    }
  }
}

variable "iso_path" {
  type = string
}

variable "output_directory" {
  type = string
}

variable "box_output_path" {
  type = string
}

variable "build_cpus" {
  type    = number
  default = 4
}

variable "build_memory_mb" {
  type    = number
  default = 8192
}

variable "headless" {
  type    = bool
  default = true
}

locals {
  iso_checksum = "sha256:cc8a95cde20f6ced61a322420de00f10cc3c90ced545daa46cb9c1a117f1d927"
}

source "virtualbox-iso" "ubuntu" {
  boot_command = [
    "<wait>e<wait><down><down><down><end> autoinstall ds=nocloud-net\\;s=http://{{ .HTTPIP }}:{{ .HTTPPort }}/<wait><f10><wait>"
  ]
  boot_wait            = "5s"
  cpus                 = var.build_cpus
  disk_size            = 81920
  guest_os_type        = "Ubuntu_64"
  hard_drive_interface = "sata"
  headless              = var.headless
  http_directory       = "${path.root}/http"
  iso_checksum         = local.iso_checksum
  iso_interface        = "sata"
  iso_url              = var.iso_path
  memory               = var.build_memory_mb
  output_directory     = var.output_directory
  shutdown_command     = "sudo /sbin/shutdown -P now"
  ssh_clear_authorized_keys = false
  # Ubuntu installation can take several hours on hosts with slow mirrors.
  ssh_handshake_attempts    = 5000
  ssh_password              = "vagrant"
  ssh_timeout               = "8h"
  ssh_username              = "vagrant"
  vm_name                   = "ubuntu-26.04.1-x86_64-base"

  vboxmanage = [
    [ "modifyvm", "{{ .Name }}", "--graphicscontroller", "vmsvga" ],
    [ "modifyvm", "{{ .Name }}", "--vram", "32" ],
    [ "modifyvm", "{{ .Name }}", "--nic1", "nat" ],
    [ "modifyvm", "{{ .Name }}", "--audio-enabled", "off" ],
    [ "modifyvm", "{{ .Name }}", "--usb", "off" ]
  ]
}

build {
  sources = [ "source.virtualbox-iso.ubuntu" ]

  provisioner "shell" {
    execute_command = "chmod +x {{ .Path }}; sudo -E {{ .Path }}"
    script          = "${path.root}/scripts/prepare-vagrant-box.sh"
  }

  post-processor "vagrant" {
    compression_level = 6
    keep_input_artifact = false
    output            = var.box_output_path
  }
}
