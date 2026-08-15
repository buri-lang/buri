function __cmd_x_main$main(){
  $host_HostStdout_println([],[__cmd_x_main$roman(1),' ',__cmd_x_main$roman(9),' ',__cmd_x_main$roman(10),' ',__cmd_x_main$roman(11)]);
  return [0,0];
}
function __cmd_x_main$roman(n_0){
  switch(n_0){
    case 1:
      return 'I';
    case 2:
      return 'II';
    case 3:
      return 'III';
    case 4:
      return 'IV';
    case 5:
      return 'V';
    case 6:
      return 'VI';
    case 7:
      return 'VII';
    case 8:
      return 'VIII';
    case 9:
      return 'IX';
    case 10:
      return 'X';
    default:
      return '?';
  }
}
