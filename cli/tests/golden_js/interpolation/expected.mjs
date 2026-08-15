function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const name_1='world';
  const n_2=42;
  core_host$HostStdout_println(ctx_0[1],['hello ',name_1]);
  core_host$HostStdout_println(ctx_0[1],[String(n_2),' and ',$f64(1.5),' and ',name_1]);
  core_host$HostStdout_println(ctx_0[1],['no holes at all']);
  const joined_4=core_str$format$72mdf3(ctx_0,['n=',String(n_2)]);
  core_host$HostStdout_println(ctx_0[1],[joined_4,joined_4]);
  return [0,0];
}
function core_host$HostStdout_println(self_0,text_1){
  return $host_HostStdout_println(self_0,text_1);
}
function core_str$format$72mdf3(ctx_0,template_1){
  return $str_format(ctx_0,template_1);
}
