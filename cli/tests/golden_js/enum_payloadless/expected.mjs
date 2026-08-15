function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],[__cmd_x_main$name(0),' ',__cmd_x_main$name(5)]);
  $host_HostStdout_println(ctx_0[1],[__cmd_x_main$name(2),' ',__cmd_x_main$name(4)]);
  return [0,0];
}
function __cmd_x_main$name(c_0){
  switch(c_0){
    case 0:
      {
        return 'red';
      }
    case 1:
      {
        return 'green';
      }
    case 2:
      {
        return 'blue';
      }
    default:
      {
        switch(c_0){
          case 3:
            {
              return 'cyan';
            }
          case 4:
            {
              return 'magenta';
            }
          case 5:
            {
              return 'yellow';
            }
          default:
            {
              $abort('no arm matched');
            }
            break;
        }
      }
      break;
  }
}
