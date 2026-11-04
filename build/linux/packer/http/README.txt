This directory is served only to the temporary Packer VM during unattended
installation. The authorized SSH key is Vagrant's published insecure
development key; no private key or project secret is stored here.
pwsh -NoProfile -ExecutionPolicy Bypass `
  -File .\_local\phase3\start-headful-box-build.ps1