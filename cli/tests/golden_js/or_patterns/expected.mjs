function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],[__cmd_x_main$kind([2]),' ',__cmd_x_main$kind([0]),' ',__cmd_x_main$kind([4,503])]);
  $host_HostStdout_println(ctx_0[1],[true,' ',false]);
  return [0,0];
}
function __cmd_x_main$kind(s_0){
  switch(s_0[0]){
    case 1:
    case 2:
      {
        return 'missing';
      }
    case 3:
    case 0:
      {
        return 'fine';
      }
    case 4:
      {
        return s_0[1]>=500?'server':'other';
      }
    default:
      {
        $abort('no arm matched');
      }
      break;
  }
}
