function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  const name_1='world';
  const n_2=42;
  $host_HostStdout_println(ctx_0[1],['hello ',name_1]);
  $host_HostStdout_println(ctx_0[1],[String(n_2),' and ',$f64(1.5),' and ',name_1]);
  $host_HostStdout_println(ctx_0[1],['no holes at all']);
  const joined_4=$str_format(ctx_0,['n=',String(n_2)]);
  $host_HostStdout_println(ctx_0[1],[joined_4,joined_4]);
  return [0,0];
}
