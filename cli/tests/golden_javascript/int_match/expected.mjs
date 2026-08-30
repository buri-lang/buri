const $k0=[0,0];
function __cmd_x_main$main(){
  $host_HostStdout_println([],__cmd_x_main$roman(1n)+' '+__cmd_x_main$roman(9n)+' '+__cmd_x_main$roman(10n)+' '+__cmd_x_main$roman(11n));
  return $k0;
}
function __cmd_x_main$roman(n_0){
  switch(n_0){
    case 1n:
      return 'I';
    case 2n:
      return 'II';
    case 3n:
      return 'III';
    case 4n:
      return 'IV';
    case 5n:
      return 'V';
    case 6n:
      return 'VI';
    case 7n:
      return 'VII';
    case 8n:
      return 'VIII';
    case 9n:
      return 'IX';
    case 10n:
      return 'X';
    default:
      return '?';
  }
}
