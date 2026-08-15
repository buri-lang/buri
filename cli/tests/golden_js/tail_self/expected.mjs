function __cmd_x_main$main(){
  const ctx_0=[[],[]];
  $host_HostStdout_println(ctx_0[1],[String(__cmd_x_main$sumTo(100,0)),' ',String(__cmd_x_main$fib(30,0,1)),' ',String(__cmd_x_main$countDigits(12345,0))]);
  $host_HostStdout_println(ctx_0[1],[String(__cmd_x_main$swapDown(1,2,3)),' ',String(__cmd_x_main$swapDown(1,2,4))]);
  return [0,0];
}
function __cmd_x_main$sumTo(n_0,acc_1){
  while(true){
    if(n_0===0){
      return acc_1;
    }else{
      const $t1=n_0-1;
      acc_1=acc_1+n_0;
      n_0=$t1;
      continue;
    }
  }
}
function __cmd_x_main$fib(n_0,a_1,b_2){
  while(true){
    if(n_0===0){
      return a_1;
    }else{
      n_0=n_0-1;
      const $t1=b_2;
      b_2=a_1+b_2;
      a_1=$t1;
      continue;
    }
  }
}
function __cmd_x_main$countDigits(n_0,acc_1){
  while(true){
    if(n_0<10){
      return acc_1+1;
    }else{
      n_0=$divi(n_0,10);
      acc_1=acc_1+1;
      continue;
    }
  }
}
function __cmd_x_main$swapDown(a_0,b_1,fuel_2){
  while(true){
    if(fuel_2===0){
      return a_0*10+b_1;
    }else{
      const $t1=b_1;
      b_1=a_0;
      fuel_2=fuel_2-1;
      a_0=$t1;
      continue;
    }
  }
}
