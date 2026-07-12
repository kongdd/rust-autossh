param([ValidateSet("install","uninstall","start","stop","restart","enable","disable","status")]$Action,[string]$Exe=".\rust-autossh.exe",[string]$Config=".\config.toml"); $n="rust-autossh"
switch ($Action) {
  "install"   { $e=(Resolve-Path $Exe).Path; $c=(Resolve-Path $Config).Path; sc.exe create $n binPath= "`"$e`" service --config `"$c`"" start= auto }
  "uninstall" { sc.exe stop $n; sc.exe delete $n }
  "restart"   { sc.exe stop $n; Start-Sleep 1; sc.exe start $n }
  "enable"    { sc.exe config $n start= auto }
  "disable"   { sc.exe config $n start= disabled }
  "status"    { sc.exe query $n }
  default      { sc.exe $Action $n }
}

# cargo build --target x86_64-pc-windows-gnu --release
