function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],[$f64(__cmd_x_main$area([0]))]);
  $host_HostStdout_println(ctx_0[1],[$f64(__cmd_x_main$area([1,2]))]);
  $host_HostStdout_println(ctx_0[1],[$f64(__cmd_x_main$area([2,3,4]))]);
  $host_HostStdout_println(ctx_0[1],[$f64(__cmd_x_main$area([3,5]))]);
  return [0,0];
}
function __cmd_x_main$area(s_0){
  if(s_0[0]===0){
    return 0;
  }else{
    switch(s_0[0]){
      case 1:
        {
          const r_1=s_0[1];
          return 3*r_1*r_1;
        }
      case 2:
        {
          return s_0[1]*s_0[2];
        }
      case 3:
        {
          const side_4=s_0[1];
          return side_4*side_4;
        }
      default:
        {
          $abort('no arm matched');
        }
        break;
    }
  }
}
