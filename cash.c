#include <stdlib.h>
#include <stdio.h>
#include <sys/wait.h>
#include <unistd.h>
#include <string.h>


#define CASH_RL_BUFSIZE 1024

char *cash_read_line(void) {
  int bufsize = CASH_RL_BUFSIZE;
  int position = 0;
  char *buffer = malloc(sizeof(char) * bufsize);
  int c;

  if (!buffer) {
    fprintf(stderr, "cash: allocation error\n");
    exit(EXIT_FAILURE);
  }

  while (1) {
    // Read a character
    c = getchar();

    // If we hit EOF, replace it with a null character and return.
    if (c == EOF || c == '\n') {
      buffer[position] = '\0';
      return buffer;
    } else {
      buffer[position] = c;
    }
    position++;

    // If we have exceeded the buffer, reallocate.
    if (position >= bufsize) {
      bufsize += CASH_RL_BUFSIZE;
      buffer = realloc(buffer, bufsize);
      if (!buffer) {
        fprintf(stderr, "cash: allocation error\n");
        exit(EXIT_FAILURE);
      }
    }
  }
}


#define CASH_TOK_BUFSIZE 64
#define CASH_TOK_DELIM " \t\r\n\a"

char **cash_split_line(char *line)
{
  int bufsize = CASH_TOK_BUFSIZE, position = 0;
  char **tokens = malloc(bufsize * sizeof(char*));
  char *token;

  if (!tokens) {
    fprintf(stderr, "cash: allocation error\n");
    exit(EXIT_FAILURE);
  }

  token = strtok(line, CASH_TOK_DELIM);
  while (token != NULL) {
    tokens[position] = token;
    position++;

    if (position >= bufsize) {
      bufsize += CASH_TOK_BUFSIZE;
      tokens = realloc(tokens, bufsize * sizeof(char*));
      if (!tokens) {
        fprintf(stderr, "cash: allocation error\n");
        exit(EXIT_FAILURE);
      }
    }

    token = strtok(NULL, CASH_TOK_DELIM);
  }
  tokens[position] = NULL;
  return tokens;
}


/*
  Function Declarations for builtin shell commands:
 */
int cash_cd(char **args);
int cash_help(char **args);
int cash_exit(char **args);

/*
  List of builtin commands, followed by their corresponding functions.
 */
char *builtin_str[] = {
  "cd",
  "help",
  "exit"
};

int (*builtin_func[]) (char **) = {
  &cash_cd,
  &cash_help,
  &cash_exit
};

int cash_num_builtins() {
  return sizeof(builtin_str) / sizeof(char *);
}

/*
  Builtin function implementations.
*/
int cash_cd(char **args)
{
  if (args[1] == NULL) {
    fprintf(stderr, "cash: expected argument to \"cd\"\n");
  } else {
    if (chdir(args[1]) != 0) {
      perror("cash");
    }
  }
  return 1;
}

int cash_help(char **args)
{
  int i;
  printf("cash-shell help:\n");
  printf("Type program names and arguments, and hit enter.\n");
  printf("The following are built in:\n");

  for (i = 0; i < cash_num_builtins(); i++) {
    printf("  %s\n", builtin_str[i]);
  }

  printf("Use the man command for information on other programs.\n");
  return 1;
}

int cash_exit(char **args)
{
  return 0;
}


int cash_launch(char **args)
{
  pid_t pid, wpid;
  int status;

  pid = fork();
  if (pid == 0) {
    // Child process
    if (execvp(args[0], args) == -1) {
      perror("cash");
    }
    exit(EXIT_FAILURE); // if you reach here, it means the child process did not execute correctly
  } else if (pid < 0) {
    // Error forking
    perror("cash");
  } else {
    // Parent process
    do {
      wpid = waitpid(pid, &status, WUNTRACED);
    } while (!WIFEXITED(status) && !WIFSIGNALED(status));
  }

  return 1;
}


int cash_execute(char **args)
{
  int i;

  if (args[0] == NULL) {
    // An empty command was entered.
    return 1;
  }

  for (i = 0; i < cash_num_builtins(); i++) {
    if (strcmp(args[0], builtin_str[i]) == 0) {
      return (*builtin_func[i])(args);
    }
  }

  return cash_launch(args);
}


void cash_loop() {
    char *line;
    char **args;
    int status;

    do {
        printf("$_$ ");
        line = cash_read_line();
        args = cash_split_line(line);
        status = cash_execute(args);

        free(line);
        free(args);
    } while (status);
}


int main(int argc, char **argv)
{
  // Load config files

  // Run command loop.
  cash_loop();

  // Perform shutdown/cleanup.

  return EXIT_SUCCESS;
}
